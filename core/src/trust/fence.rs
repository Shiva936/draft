//! `TrustReadFence` — a stable view of the trust registry across a decision.
//!
//! Trust changes. A key is revoked, a publisher is delisted, an authority is
//! withdrawn — and each of those is meant to affect what Draft will do *next*,
//! not to silently change the meaning of a decision already in flight.
//!
//! Without a fence, a Promotion could resolve its security facts, find them
//! trusted, and commit after a revocation had already landed. The commit would
//! be well formed and its recorded provenance would be a lie: it would name
//! registry revisions that were no longer current when it committed.
//!
//! The fence closes that window by holding the registry stable across the
//! decision:
//!
//! ```text
//! acquire the fence                      (order 1 — nothing else is held yet)
//! read the registry revisions            <- inside the fence
//! ... resolve security facts, evaluate gates, validate the commit ...
//! re-read the revisions and compare      <- still inside the fence
//! commit
//! release
//! ```
//!
//! # Why it is order 1
//!
//! It is global, so everything else nests inside it. A path that took a project
//! or publication lock first and then reached for the fence could deadlock
//! against one that took them in the other order — which is why the partial
//! order puts it first and why no Publication path may acquire it while holding
//! a lease.
//!
//! # What it is not
//!
//! It is not a write lock on trust, and it does not stop a registry writer from
//! existing — it stops one from landing *inside* a decision. A writer waits, or
//! the decision it would have invalidated fails its comparison and is refused.
//! Either outcome is honest; a commit against a registry that moved underneath
//! it is not.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use draft_dcg_contract::value::{RegistryId, RegistryRevision, RegistryRevisions};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::lock_order::LockOrder;
use crate::support::process_lock::ProcessFileLock;

/// How long a reader waits for the fence.
pub const FENCE_TIMEOUT: Duration = Duration::from_secs(30);

/// The registry file, relative to the global store.
pub const REGISTRY_FILE: &str = "trust/registry.json";
/// The stable sidecar the fence is held on.
pub const REGISTRY_LOCK: &str = "trust/registry.lock";

/// A held, stable view of the trust registry.
///
/// Released on drop, and — because the underlying lock is kernel-owned — the
/// instant the holding process dies.
#[derive(Debug)]
pub struct TrustReadFence {
    global_store: PathBuf,
    // Held for its side effect.
    #[allow(dead_code)]
    lock: ProcessFileLock,
}

impl TrustReadFence {
    /// Acquire the fence over `global_store`.
    pub fn acquire(global_store: impl Into<PathBuf>, timeout: Duration) -> DraftResult<Self> {
        let global_store = global_store.into();
        let lock = ProcessFileLock::acquire_exclusive_ordered(
            &global_store.join(REGISTRY_LOCK),
            timeout,
            LockOrder::TrustReadFence,
        )?;
        Ok(Self { global_store, lock })
    }

    /// The registry revisions as they stand right now.
    ///
    /// Meaningful only while the fence is held: that is what makes two reads
    /// inside one fence comparable, and what makes a difference between them
    /// evidence of a bypass rather than ordinary concurrency.
    pub fn observed_revisions(&self) -> DraftResult<RegistryRevisions> {
        let path = self.global_store.join(REGISTRY_FILE);
        if !path.exists() {
            // No registry is a legitimate state — a project with nothing
            // registered has nothing to observe — and is distinct from a
            // registry that failed to load.
            return Ok(RegistryRevisions::new());
        }
        let bytes = std::fs::read(&path).map_err(|error| {
            DraftError::storage(format!(
                "cannot read the trust registry {}: {error}",
                path.display()
            ))
        })?;
        let raw: BTreeMap<String, u64> = serde_json::from_slice(&bytes).map_err(|error| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!("the trust registry is unreadable: {error}"),
            )
        })?;

        let mut revisions = RegistryRevisions::new();
        for (id, revision) in raw {
            let registry = RegistryId::parse(&id).map_err(|error| {
                DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!("the trust registry names '{id}', which is not a registry id: {error}"),
                )
            })?;
            revisions.insert(registry, RegistryRevision::new(revision));
        }
        Ok(revisions)
    }

    /// Require the registry to be exactly what a decision was planned against.
    ///
    /// Called inside the fence, immediately before the commit. A difference
    /// means the registry moved between planning and committing — which the
    /// fence should have prevented, so it indicates a bypass rather than
    /// ordinary concurrency, and the decision is refused rather than adjusted.
    pub fn require_unchanged(&self, planned: &RegistryRevisions) -> DraftResult<()> {
        let current = self.observed_revisions()?;
        if &current != planned {
            return Err(DraftError::new(
                DraftErrorKind::RegistryStale,
                "the trust registry changed while a decision was being taken against it"
                    .to_string(),
            )
            .with_suggestion(
                "Re-evaluate against the current registry; a decision is never committed \
                 against trust state that has since moved.",
            ));
        }
        Ok(())
    }

    /// The global store this fence covers.
    pub fn global_store(&self) -> &Path {
        &self.global_store
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::lock_order;

    fn write_registry(store: &Path, entries: &[(&str, u64)]) {
        let path = store.join(REGISTRY_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let map: BTreeMap<&str, u64> = entries.iter().copied().collect();
        std::fs::write(&path, serde_json::to_vec(&map).unwrap()).unwrap();
    }

    fn revisions(entries: &[(&str, u64)]) -> RegistryRevisions {
        entries
            .iter()
            .map(|(id, revision)| {
                (
                    RegistryId::parse(id).unwrap(),
                    RegistryRevision::new(*revision),
                )
            })
            .collect()
    }

    #[test]
    fn an_absent_registry_is_an_empty_observation_not_an_error() {
        // A project with nothing registered has nothing to observe, which is
        // different from a registry that failed to load.
        let directory = tempfile::tempdir().unwrap();
        let fence = TrustReadFence::acquire(directory.path(), FENCE_TIMEOUT).unwrap();
        assert!(fence.observed_revisions().unwrap().is_empty());
    }

    #[test]
    fn the_registry_is_read_as_observed_revisions() {
        let directory = tempfile::tempdir().unwrap();
        write_registry(
            directory.path(),
            &[
                ("draft.trust/publishers", 412),
                ("draft.trust/authorities", 7),
            ],
        );
        let fence = TrustReadFence::acquire(directory.path(), FENCE_TIMEOUT).unwrap();
        assert_eq!(
            fence.observed_revisions().unwrap(),
            revisions(&[
                ("draft.trust/publishers", 412),
                ("draft.trust/authorities", 7)
            ])
        );
    }

    #[test]
    fn an_unchanged_registry_permits_the_commit() {
        let directory = tempfile::tempdir().unwrap();
        write_registry(directory.path(), &[("draft.trust/publishers", 412)]);
        let fence = TrustReadFence::acquire(directory.path(), FENCE_TIMEOUT).unwrap();
        let planned = fence.observed_revisions().unwrap();
        fence.require_unchanged(&planned).unwrap();
    }

    #[test]
    fn a_registry_that_moved_refuses_the_commit() {
        // The window the fence exists to close. If this were tolerated, the
        // commit's recorded provenance would name revisions that were no
        // longer current when it landed.
        let directory = tempfile::tempdir().unwrap();
        write_registry(directory.path(), &[("draft.trust/publishers", 412)]);
        let fence = TrustReadFence::acquire(directory.path(), FENCE_TIMEOUT).unwrap();
        let planned = fence.observed_revisions().unwrap();

        // A bypass: only something ignoring the fence could do this.
        write_registry(directory.path(), &[("draft.trust/publishers", 413)]);

        let error = fence.require_unchanged(&planned).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::RegistryStale);
    }

    #[test]
    fn a_revocation_that_removes_a_registry_is_also_a_change() {
        // Not only advancing revisions counts: a registry disappearing changes
        // what a decision was evaluated against just as much.
        let directory = tempfile::tempdir().unwrap();
        write_registry(
            directory.path(),
            &[
                ("draft.trust/publishers", 412),
                ("draft.trust/authorities", 7),
            ],
        );
        let fence = TrustReadFence::acquire(directory.path(), FENCE_TIMEOUT).unwrap();
        let planned = fence.observed_revisions().unwrap();

        write_registry(directory.path(), &[("draft.trust/publishers", 412)]);
        assert!(fence.require_unchanged(&planned).is_err());
    }

    #[test]
    fn a_second_reader_waits_rather_than_seeing_a_moving_registry() {
        let directory = tempfile::tempdir().unwrap();
        let held = TrustReadFence::acquire(directory.path(), FENCE_TIMEOUT).unwrap();
        let contended = TrustReadFence::acquire(directory.path(), Duration::from_millis(200));
        assert_eq!(
            contended.unwrap_err().kind,
            DraftErrorKind::LockTimeout,
            "the fence must not be granted twice at once"
        );
        drop(held);
        TrustReadFence::acquire(directory.path(), FENCE_TIMEOUT).unwrap();
    }

    #[test]
    fn the_fence_is_outermost_in_the_lock_order() {
        // Everything else nests inside it. A path that took a project or
        // publication lock first and then reached for the fence could deadlock
        // against one that took them in the other order.
        let directory = tempfile::tempdir().unwrap();
        let _fence = TrustReadFence::acquire(directory.path(), FENCE_TIMEOUT).unwrap();
        assert_eq!(
            lock_order::currently_held(),
            vec![LockOrder::TrustReadFence]
        );
        // Nothing may be acquired before it, because nothing is before it.
        lock_order::enter(LockOrder::ProjectControlStore).unwrap();
    }

    #[test]
    fn a_lease_holder_may_not_reach_for_the_fence() {
        let _lease = lock_order::enter(LockOrder::PublicationLease).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let error = TrustReadFence::acquire(directory.path(), FENCE_TIMEOUT).unwrap_err();
        assert!(
            error.message.contains("reverse lock acquisition"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_corrupt_registry_is_reported_rather_than_read_as_empty() {
        // Treating it as empty would let a decision commit claiming it observed
        // no trust state at all.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(REGISTRY_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{not json").unwrap();

        let fence = TrustReadFence::acquire(directory.path(), FENCE_TIMEOUT).unwrap();
        assert_eq!(
            fence.observed_revisions().unwrap_err().kind,
            DraftErrorKind::CorruptData
        );
    }

    #[test]
    fn a_registry_naming_something_that_is_not_a_registry_id_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(REGISTRY_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, br#"{"unowned":1}"#).unwrap();

        let fence = TrustReadFence::acquire(directory.path(), FENCE_TIMEOUT).unwrap();
        assert_eq!(
            fence.observed_revisions().unwrap_err().kind,
            DraftErrorKind::CorruptData
        );
    }
}
