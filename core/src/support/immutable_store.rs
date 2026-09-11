//! Create-once storage for immutable facts.
//!
//! The exact-reference rule says *which* references must carry a digest. It is
//! not the whole guarantee: a canonical parent often refers to an immutable
//! fact by **logical id alone**, because the digest does not need to travel in
//! that contract. `Evidence` names a `ChangeRevisionId`; a `GateEvaluation`
//! names a `DecisionId`. Nothing in those references would notice if the bytes
//! beneath the id were replaced.
//!
//! So one storage-level rule applies beneath the reference rule:
//!
//! ```text
//! LogicalId  ->  CanonicalPayloadDigest  ->  immutable canonical payload
//!
//! The binding is written ONCE and may NEVER be replaced with a different digest.
//! ```
//!
//! and every load recomputes the payload's digest and compares it against that
//! binding. A mismatch is an integrity violation — never a warning, and never
//! repaired in place.
//!
//! # What this makes the word "exact" mean
//!
//! When Draft says Evidence binds an *exact* `ChangeRevisionId`, that is
//! mechanical rather than rhetorical: the id is backed by a create-once binding
//! verified on every read, so "exact revision" cannot degrade into "the same
//! opaque `rev_` string". No separate `ChangeRevisionDigest` type is needed,
//! because no portable contract requires that digest to travel.
//!
//! # The Store invariant
//!
//! ```text
//! same logical id + byte-identical canonical payload  -> idempotent success
//! same logical id + a DIFFERENT canonical payload     -> integrity violation
//! ```
//!
//! Rewriting a fact with identical bytes is a no-op — that is what makes crash
//! recovery able to retry a write without knowing whether it already landed.
//! Rewriting it with *different* bytes is the thing being prevented. There is
//! no last-writer-wins immutable-fact storage anywhere in Draft.

use std::marker::PhantomData;
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::try_canonical_hash;

/// What storing a fact did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOutcome {
    /// The fact was written for the first time.
    Created,
    /// The fact was already stored with byte-identical canonical bytes.
    ///
    /// Not an error: a retried write after an uncertain crash must be able to
    /// converge without the caller knowing whether the first attempt landed.
    AlreadyIdentical,
}

/// A create-once store for one family of immutable facts.
#[derive(Debug, Clone)]
pub struct ImmutableFactStore<T> {
    directory: PathBuf,
    marker: PhantomData<T>,
}

impl<T: Serialize + DeserializeOwned> ImmutableFactStore<T> {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            marker: PhantomData,
        }
    }

    /// Where the canonical payload lives.
    pub fn payload_path(&self, logical_id: &str) -> PathBuf {
        self.directory.join(format!("{logical_id}.json"))
    }

    /// Where the create-once digest binding lives.
    ///
    /// A separate file, created exclusively, so the binding's immutability is
    /// enforced by the filesystem rather than by remembering to check.
    pub fn binding_path(&self, logical_id: &str) -> PathBuf {
        self.directory.join(format!("{logical_id}.digest"))
    }

    /// The digest this logical id is bound to, if it exists.
    pub fn bound_digest(&self, logical_id: &str) -> DraftResult<Option<String>> {
        let path = self.binding_path(logical_id);
        if !path.exists() {
            return Ok(None);
        }
        let digest = std::fs::read_to_string(&path).map_err(|error| {
            DraftError::storage(format!("cannot read binding {}: {error}", path.display()))
        })?;
        Ok(Some(digest.trim().to_string()))
    }

    /// Store `value` under `logical_id`.
    ///
    /// Idempotent for identical bytes; an integrity violation for anything
    /// else.
    pub fn put(&self, logical_id: &str, value: &T) -> DraftResult<StoreOutcome> {
        let digest = try_canonical_hash(value)?;

        if let Some(bound) = self.bound_digest(logical_id)? {
            if bound != digest {
                crate::support::telemetry::Counter::ImmutableFactIntegrityViolations.increment();
                return Err(DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!(
                        "immutable fact '{logical_id}' is bound to {bound}, so it cannot be \
                         rewritten as {digest}"
                    ),
                )
                .with_suggestion(
                    "An immutable fact's bytes never change. Mint a new identity instead.",
                ));
            }
            // Same id, same bytes: converge silently. The payload is rewritten
            // in case a previous attempt crashed between the two writes.
            self.write_payload(logical_id, value)?;
            return Ok(StoreOutcome::AlreadyIdentical);
        }

        // Payload first, then the binding. A crash between them leaves a
        // payload with no binding, which `get` refuses and `put` can complete —
        // whereas a binding with no payload would be a dangling promise.
        self.write_payload(logical_id, value)?;
        self.create_binding(logical_id, &digest)?;
        Ok(StoreOutcome::Created)
    }

    /// Every logical id this store holds a payload for.
    ///
    /// Reads the directory rather than an index. A second place the set of
    /// facts lives could disagree with the first, and a fact missing from an
    /// index would be invisible to a reader enumerating them — which for a
    /// read model means silently answering a question with less than the
    /// project knows.
    pub fn list_ids(&self) -> DraftResult<Vec<String>> {
        let entries = match std::fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(DraftError::storage(format!(
                    "cannot list facts in {}: {error}",
                    self.directory.display()
                )))
            }
        };
        let mut found = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|value| value == "json") {
                if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                    found.push(stem.to_string());
                }
            }
        }
        found.sort();
        Ok(found)
    }

    /// Load `logical_id`, verifying its bytes against the create-once binding.
    pub fn get(&self, logical_id: &str) -> DraftResult<Option<T>> {
        let payload_path = self.payload_path(logical_id);
        if !payload_path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&payload_path).map_err(|error| {
            DraftError::storage(format!(
                "cannot read fact {}: {error}",
                payload_path.display()
            ))
        })?;
        let value: T = serde_json::from_slice(&bytes).map_err(|error| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!("cannot decode fact {}: {error}", payload_path.display()),
            )
        })?;

        let Some(bound) = self.bound_digest(logical_id)? else {
            crate::support::telemetry::Counter::ImmutableFactIntegrityViolations.increment();
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "immutable fact '{logical_id}' has a payload but no digest binding, so its \
                     contents cannot be verified"
                ),
            ));
        };
        let actual = try_canonical_hash(&value)?;
        if actual != bound {
            crate::support::telemetry::Counter::ImmutableFactIntegrityViolations.increment();
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "immutable fact '{logical_id}' is bound to {bound} but its stored bytes \
                     compute to {actual}"
                ),
            )
            .with_suggestion("Run `draft doctor`; stored history must not be repaired in place."));
        }
        Ok(Some(value))
    }

    fn write_payload(&self, logical_id: &str, value: &T) -> DraftResult<()> {
        let encoded = serde_json::to_vec_pretty(value).map_err(|error| {
            DraftError::storage(format!("cannot encode fact '{logical_id}': {error}"))
        })?;
        crate::support::fsutil::write_atomic(&self.payload_path(logical_id), &encoded)
    }

    fn create_binding(&self, logical_id: &str, digest: &str) -> DraftResult<()> {
        use std::io::Write as _;

        let path = self.binding_path(logical_id);
        // `create_new` is the enforcement: the filesystem refuses a second
        // creation, so the binding cannot be replaced even by code that
        // forgot it must not be.
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // Another writer bound it between our check and now. Identical
                // bytes converge; anything else is caught on the next read.
                return Ok(());
            }
            Err(error) => {
                return Err(DraftError::storage(format!(
                    "cannot create binding {}: {error}",
                    path.display()
                )))
            }
        };
        file.write_all(digest.as_bytes()).map_err(|error| {
            DraftError::storage(format!("cannot write binding {}: {error}", path.display()))
        })?;
        file.sync_all().map_err(|error| {
            DraftError::storage(format!("cannot sync binding {}: {error}", path.display()))
        })?;
        if let Some(parent) = path.parent() {
            crate::support::fsutil::sync_directory(parent)?;
        }
        Ok(())
    }
}

/// A reusable conformance harness for immutable-fact stores.
///
/// Every immutable Store is expected to hold the same two properties, so they
/// are asserted from one place rather than restated per Store — a duplicated
/// assertion is one that eventually gets duplicated *wrongly*.
///
/// `first` and `second` must be different values. Panics on any violation, so
/// a caller is a single line in a test.
pub fn assert_immutable_fact_store_conformance<T>(
    store: &ImmutableFactStore<T>,
    logical_id: &str,
    first: &T,
    second: &T,
) where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    assert_ne!(
        try_canonical_hash(first).unwrap(),
        try_canonical_hash(second).unwrap(),
        "the conformance harness needs two genuinely different values"
    );

    assert_eq!(
        store
            .put(logical_id, first)
            .expect("first write must succeed"),
        StoreOutcome::Created
    );
    assert_eq!(
        store.get(logical_id).expect("must load back").as_ref(),
        Some(first)
    );

    // Same id, same bytes: idempotent, so a retried write after an uncertain
    // crash converges instead of failing.
    assert_eq!(
        store
            .put(logical_id, first)
            .expect("an identical rewrite must be idempotent"),
        StoreOutcome::AlreadyIdentical
    );

    // Same id, different bytes: refused.
    let error = store
        .put(logical_id, second)
        .expect_err("an immutable fact must not be rewritten with different bytes");
    assert_eq!(
        error.kind,
        DraftErrorKind::CorruptData,
        "rewriting an immutable fact must be an integrity failure"
    );

    // And the original survives the attempt.
    assert_eq!(
        store.get(logical_id).expect("must still load").as_ref(),
        Some(first),
        "a refused rewrite must not have altered the stored fact"
    );

    // Content substitution behind the id is detected on load.
    let substituted = serde_json::to_vec_pretty(second).unwrap();
    std::fs::write(store.payload_path(logical_id), substituted).unwrap();
    let error = store
        .get(logical_id)
        .expect_err("id-preserving content substitution must be detected");
    assert_eq!(error.kind, DraftErrorKind::CorruptData);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Decision {
        revision: String,
        outcome: String,
    }

    fn decision(outcome: &str) -> Decision {
        Decision {
            revision: "rev_a1".into(),
            outcome: outcome.into(),
        }
    }

    fn store(directory: &tempfile::TempDir) -> ImmutableFactStore<Decision> {
        ImmutableFactStore::new(directory.path())
    }

    #[test]
    fn the_conformance_harness_passes_for_a_correct_store() {
        let directory = tempfile::tempdir().unwrap();
        assert_immutable_fact_store_conformance(
            &store(&directory),
            "dec_a1",
            &decision("approved"),
            &decision("rejected"),
        );
    }

    #[test]
    fn a_binding_cannot_be_replaced_even_by_direct_write() {
        // `create_new` means the filesystem refuses, so the guarantee does not
        // depend on every future caller remembering the rule.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.put("dec_a1", &decision("approved")).unwrap();

        let binding = store.binding_path("dec_a1");
        assert!(std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&binding)
            .is_err());
    }

    #[test]
    fn a_payload_without_a_binding_is_refused_rather_than_trusted() {
        // The crash window between the two writes. Returning the payload here
        // would mean serving an unverifiable fact.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        std::fs::create_dir_all(directory.path()).unwrap();
        std::fs::write(
            store.payload_path("dec_orphan"),
            serde_json::to_vec(&decision("approved")).unwrap(),
        )
        .unwrap();

        let error = store.get("dec_orphan").unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn a_crashed_write_can_be_completed_by_retrying() {
        // Following on: the same call that failed can be repeated, and it
        // converges rather than reporting a conflict against itself.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        std::fs::create_dir_all(directory.path()).unwrap();
        std::fs::write(
            store.payload_path("dec_a1"),
            serde_json::to_vec_pretty(&decision("approved")).unwrap(),
        )
        .unwrap();

        assert_eq!(
            store.put("dec_a1", &decision("approved")).unwrap(),
            StoreOutcome::Created
        );
        assert_eq!(store.get("dec_a1").unwrap(), Some(decision("approved")));
    }

    #[test]
    fn an_absent_fact_is_absent_rather_than_an_error() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(store(&directory).get("dec_missing").unwrap(), None);
    }

    #[test]
    fn two_facts_do_not_share_a_binding() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.put("dec_a1", &decision("approved")).unwrap();
        store.put("dec_b2", &decision("rejected")).unwrap();
        assert_ne!(
            store.bound_digest("dec_a1").unwrap(),
            store.bound_digest("dec_b2").unwrap()
        );
        assert_eq!(store.get("dec_a1").unwrap(), Some(decision("approved")));
        assert_eq!(store.get("dec_b2").unwrap(), Some(decision("rejected")));
    }

    #[test]
    fn a_field_reordering_is_not_a_substitution() {
        // The binding is over the *canonical* form, so a byte-level rewrite
        // that preserves the canonical value is correctly accepted. Otherwise
        // a serializer change would look like tampering.
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.put("dec_a1", &decision("approved")).unwrap();
        std::fs::write(
            store.payload_path("dec_a1"),
            br#"{"outcome":"approved","revision":"rev_a1"}"#,
        )
        .unwrap();
        assert_eq!(store.get("dec_a1").unwrap(), Some(decision("approved")));
    }
}
