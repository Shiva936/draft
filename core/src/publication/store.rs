//! Creating a Publication: one request key, one Publication, forever.
//!
//! [`crate::publication::registry`] decides what a creation transaction means.
//! This is where that decision meets durable storage, and it is the only way a
//! Publication comes into existence.
//!
//! # Why creation has to be idempotent at all
//!
//! A Publication is an authorization to affect the outside world. A user who
//! runs `draft publish` twice — or once, over a connection that dropped before
//! the reply — must not end up with two of them. So the identity is *derived*
//! from what the request is about rather than minted fresh: the same promotion,
//! Baseline, route and purpose recompute the same request key, the same key
//! recomputes the same id, and the second request converges on the first
//! Publication instead of creating a rival.
//!
//! That is why no surface passes a Publication id in. There is nothing for a
//! client to generate, and therefore nothing for a client's retry logic to get
//! wrong.
//!
//! # Why the mapping is the create-once binding
//!
//! §2.45 storage already keeps a `logical id → canonical digest` binding that
//! is written once and re-verified on load. That is exactly the
//! `request_key → PublicationRef` mapping the registry requires, including the
//! digest half: bytes that no longer recompute to the bound digest fail the
//! load rather than resolving to a Publication for a different route.
//!
//! Keeping a second mapping file beside it would be a second place the same
//! fact lives, and the interesting failure is the two disagreeing.
//!
//! # Why no creation journal yet
//!
//! The registry's journal dimension exists to make the `PublicationRequested`
//! activity fact drainable exactly once. Nothing emits that fact yet, so there
//! is no third write to be interrupted between: creation is object-then-binding
//! inside one create-once store, and an interrupted creation leaves an
//! unmapped orphan that a retry completes.
//!
//! So this passes [`CreationJournalState::Prepared`] — the honest statement
//! that nothing has ever marked this transaction committed — and lets the
//! mapping decide. When the fact is emitted, the journal becomes a real record
//! and the `Committed`-with-no-mapping branch becomes reachable.

use std::path::PathBuf;

use draft_dcg_contract::ids::PublicationId;
use draft_dcg_contract::publication::{
    Publication, PublicationAttempt, PublicationAttemptRef, PublicationRequestKey,
};

use crate::publication::registry::{
    classify_creation, CreationJournalState, CreationOutcome, MappedPublication,
};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::immutable_store::ImmutableFactStore;

/// The Publications this project has requested.
#[derive(Debug, Clone)]
pub struct PublicationStore {
    facts: ImmutableFactStore<Publication>,
    directory: PathBuf,
}

impl PublicationStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        let directory = directory.into();
        Self {
            facts: ImmutableFactStore::new(directory.clone()),
            directory,
        }
    }

    pub fn for_layout(layout: &crate::project::layout::DraftLayout) -> Self {
        Self::new(layout.publications_dir())
    }

    /// Create this Publication, or converge on the one its key already names.
    ///
    /// The candidate's own `request_key` decides its id, so a caller cannot
    /// present the same request under a different identity.
    pub fn create(&self, candidate: &Publication) -> DraftResult<CreationOutcome> {
        let format = |error: draft_dcg_contract::FormatError| {
            DraftError::new(DraftErrorKind::Validation, error.to_string())
        };
        candidate.validate().map_err(format)?;

        let expected = id_for_request_key(&candidate.request_key)?;
        if candidate.id != expected {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "publication '{}' does not carry the id its request key derives ({expected}), \
                     so the same request could be created twice under two identities",
                    candidate.id
                ),
            ));
        }
        let reference = candidate.reference().map_err(format)?;

        // What the registry already holds for this key, read rather than
        // assumed. A load failure here is an absent object, not a corrupt
        // mapping: the create-once binding raises corruption itself.
        let stored = self.facts.get(candidate.id.as_str())?;
        let stored_reference = match &stored {
            Some(publication) => Some(publication.reference().map_err(format)?),
            None => None,
        };

        let outcome = classify_creation(
            CreationJournalState::Prepared,
            match &stored_reference {
                Some(reference) => MappedPublication::Present(reference),
                None => MappedPublication::Absent,
            },
            &reference,
            stored_reference.as_ref().map(|value| &value.digest),
        );

        // The registry's uniqueness rule at work: this key already names a
        // different Publication, or the bytes beneath it no longer verify.
        // Either way the candidate never became authoritative.
        if matches!(
            outcome,
            CreationOutcome::Existing(_) | CreationOutcome::Corrupt { .. }
        ) {
            crate::support::telemetry::Counter::PublicationRegistryConflicts.increment();
        }
        if let CreationOutcome::Created(_) = &outcome {
            // Create-once: identical bytes converge, different bytes under the
            // same id are an integrity failure rather than an overwrite.
            self.facts.put(candidate.id.as_str(), candidate)?;
        }
        Ok(outcome)
    }

    /// Load a Publication, verified against its create-once binding.
    pub fn get(&self, id: &PublicationId) -> DraftResult<Option<Publication>> {
        self.facts.get(id.as_str())
    }

    /// Every Publication this project has requested.
    ///
    /// Reads the payload directory rather than an index, so a Publication can
    /// never exist while being invisible to a caller enumerating them.
    pub fn list(&self) -> DraftResult<Vec<PublicationId>> {
        let entries = match std::fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(DraftError::storage(format!(
                    "cannot list publications in {}: {error}",
                    self.directory.display()
                )))
            }
        };
        let mut found = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|value| value == "json") {
                if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                    if let Ok(id) = PublicationId::parse(stem) {
                        found.push(id);
                    }
                }
            }
        }
        found.sort();
        Ok(found)
    }
}

/// The attempts this project has dispatched, written once.
///
/// A `PublicationAttempt` is the immutable claim that *this* effect happened
/// under *that* authority at *that* moment. It is stored so a verifier — and
/// [`crate::publication::consistency`] — can check it against the Publication
/// it claims and the journal that dispatched it, which no digest over the
/// attempt alone can do: a digest proves the bytes did not change, never that
/// they agreed with anything else.
#[derive(Debug, Clone)]
pub struct PublicationAttemptStore {
    facts: ImmutableFactStore<PublicationAttempt>,
}

impl PublicationAttemptStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            facts: ImmutableFactStore::new(directory),
        }
    }

    pub fn for_layout(layout: &crate::project::layout::DraftLayout) -> Self {
        Self::new(layout.publication_attempts_dir())
    }

    /// Store an attempt and return the exact reference to it.
    ///
    /// The reference carries the real canonical digest, so everything
    /// downstream — the journal's dispatch boundary, the primary outcome, the
    /// consistency checks — names these exact bytes rather than an identity
    /// derived from the id.
    pub fn put(&self, attempt: &PublicationAttempt) -> DraftResult<PublicationAttemptRef> {
        let reference = attempt
            .reference()
            .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))?;
        self.facts.put(attempt.id.as_str(), attempt)?;
        Ok(reference)
    }

    /// Every attempt artifact this project holds.
    ///
    /// Reads the payload directory rather than an index, because an artifact
    /// invisible to an enumerating reader is an artifact garbage collection
    /// would treat as absent — and a staged attempt looks like garbage from
    /// every angle except the journal that names it.
    pub fn list(&self) -> DraftResult<Vec<draft_dcg_contract::ids::PublicationAttemptId>> {
        let mut found = Vec::new();
        for id in self.facts.list_ids()? {
            if let Ok(attempt) = draft_dcg_contract::ids::PublicationAttemptId::parse(&id) {
                found.push(attempt);
            }
        }
        found.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        Ok(found)
    }

    /// Load an attempt, verified against the reference that names it.
    pub fn get(
        &self,
        reference: &PublicationAttemptRef,
    ) -> DraftResult<Option<PublicationAttempt>> {
        let Some(attempt) = self.facts.get(reference.id.as_str())? else {
            return Ok(None);
        };
        attempt.verify_reference(reference).map_err(|error| {
            crate::support::telemetry::Counter::PublicationAttemptDigestMismatches.increment();
            DraftError::new(DraftErrorKind::CorruptData, error.to_string())
        })?;
        Ok(Some(attempt))
    }
}

/// The id a Publication's request key derives.
///
/// Derived rather than minted so that two requests for the same delivery of
/// the same Baseline to the same route are the same Publication by
/// construction, and a retry has nothing to generate differently.
pub fn id_for_request_key(request_key: &PublicationRequestKey) -> DraftResult<PublicationId> {
    let short: String = request_key
        .digest()
        .as_str()
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .chars()
        .take(24)
        .collect();
    PublicationId::parse(format!("pub_{short}"))
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}
