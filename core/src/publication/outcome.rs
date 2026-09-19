//! The primary outcome of one attempt, recorded at most once.
//!
//! # Why the object is durable before the head exists
//!
//! `record_once` writes in a frozen order: the immutable outcome object
//! first, then the head that points at it. The two crash windows are not
//! symmetric.
//!
//! ```text
//! object written, head missing   → an orphan. Nothing references it, a retry
//!                                  finds the head absent and reuses the exact
//!                                  identical object, and GC can collect it.
//! head written, object missing   → a dangling head. Something authoritative
//!                                  points at a fact that does not exist, and
//!                                  no retry can repair it.
//! ```
//!
//! An orphan is recoverable and a dangling head is not, so the order is the
//! one that can only produce orphans.
//!
//! # Why a conflict is an integrity violation, not concurrency
//!
//! Two workers cannot legitimately reach `record_once` with different
//! candidates for the same attempt. Candidate selection is serialized by the
//! attempt journal: whoever holds the guard writes `OutcomePrepared`, and the
//! next holder *observes* that candidate rather than choosing another. So a
//! head that already exists with different bytes cannot have come from
//! ordinary contention — it implies a direct-write bypass of the journal.
//!
//! Treating it as concurrency would mean "converging" by picking one, which
//! silently discards a recorded claim about whether an external effect
//! occurred. [`RecordOnce::Conflict`] is therefore surfaced for hard recovery
//! and never resolved automatically.

use draft_dcg_contract::ids::{PublicationAttemptId, ReceiptId};
use draft_dcg_contract::publication::{
    PublicationAttemptRef, PublicationOutcome, PublicationOutcomeDigest,
};
use draft_dcg_contract::receipt::ReceiptSignerBinding;

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil::write_atomic;
use crate::support::immutable_store::ImmutableFactStore;
use crate::support::lock_order::LockOrder;
use crate::support::process_lock::ProcessFileLock;
use crate::support::record_guard::DEFAULT_LOCK_TIMEOUT;

/// The identity a primary outcome must carry.
///
/// All three are frozen before the external system is ever called and copied
/// into the durable `OutcomePrepared` state, so the outcome a returning worker
/// records cannot quietly acquire a different receipt or a different signer
/// than the one the dispatch was authorized under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrimaryOutcomeIdentity {
    pub attempt: PublicationAttemptRef,
    pub receipt: ReceiptId,
    pub signer: ReceiptSignerBinding,
}

/// What `record_once` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordOnce {
    /// This call established the primary outcome.
    Created,
    /// The primary outcome already existed and is byte-identical.
    ///
    /// The ordinary retry after an uncertain crash. Idempotent, not an error.
    ExistingSame,
    /// A different primary outcome already exists for this attempt.
    ///
    /// Never resolved by choosing one. See the module documentation.
    Conflict {
        existing: PublicationOutcomeDigest,
        offered: PublicationOutcomeDigest,
    },
}

impl RecordOnce {
    /// Whether the caller may proceed to `OutcomeRecorded`.
    pub fn is_committed(&self) -> bool {
        matches!(self, Self::Created | Self::ExistingSame)
    }
}

/// Primary outcomes, one per attempt.
#[derive(Debug, Clone)]
pub struct PublicationOutcomeStore {
    objects: ImmutableFactStore<PublicationOutcome>,
    heads: std::path::PathBuf,
}

impl PublicationOutcomeStore {
    /// Open the store over `publication/outcome/`.
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        let directory = directory.into();
        Self {
            objects: ImmutableFactStore::new(directory.join("objects")),
            heads: directory.join("heads"),
        }
    }

    fn head_path(&self, attempt: &PublicationAttemptRef) -> std::path::PathBuf {
        self.heads.join(format!("{}.json", attempt.id))
    }

    /// The primary outcome of an attempt, if one has been recorded.
    ///
    /// Reads the head, then loads the object it names and verifies it against
    /// the attempt reference. A head naming an object that does not load, or
    /// that does not verify, is an integrity failure rather than an absence —
    /// "no outcome" and "an outcome we cannot read" are different answers and
    /// must never collapse into one.
    pub fn primary_outcome(
        &self,
        attempt: &PublicationAttemptRef,
    ) -> DraftResult<Option<PublicationOutcome>> {
        let head_path = self.head_path(attempt);
        let head = match std::fs::read(&head_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(DraftError::new(
                    DraftErrorKind::Storage,
                    format!(
                        "could not read the outcome head at {}: {error}",
                        head_path.display()
                    ),
                ))
            }
        };
        let digest: PublicationOutcomeDigest = serde_json::from_slice(&head).map_err(|error| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "the outcome head for '{}' is unreadable: {error}",
                    attempt.id
                ),
            )
        })?;

        let outcome = self.objects.get(&attempt.id.to_string())?.ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "attempt '{}' has an outcome head naming {digest} but no outcome object",
                    attempt.id
                ),
            )
        })?;

        let recomputed = outcome.digest().map_err(|error| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!("the outcome of '{}' does not digest: {error}", attempt.id),
            )
        })?;
        if recomputed != digest {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "attempt '{}' has an outcome head naming {digest} but its object computes to \
                     {recomputed}",
                    attempt.id
                ),
            ));
        }
        Ok(Some(outcome))
    }

    /// The stable sidecar this attempt's outcome head is locked on.
    ///
    /// A sidecar, never the head file itself: the head is replaced by atomic
    /// rename, so a lock on its inode would stop protecting anything the
    /// moment a write landed.
    pub fn head_lock_path(&self, attempt: &PublicationAttemptId) -> std::path::PathBuf {
        self.heads.join(format!("{attempt}.lock"))
    }

    /// Record the primary outcome of `attempt`, at most once.
    ///
    /// The whole read-decide-write sequence runs under this attempt's
    /// outcome-head lock (order 9), so no two recorders interleave inside it.
    /// Within that section the physical order is frozen: object first, head
    /// second. A caller reaches this only from a durable `OutcomePrepared` —
    /// nothing calls it straight out of `Dispatching`.
    pub fn record_once(
        &self,
        identity: &PrimaryOutcomeIdentity,
        candidate: &PublicationOutcome,
    ) -> DraftResult<RecordOnce> {
        let attempt = &identity.attempt;
        candidate
            .validate_under(attempt, &identity.receipt, &identity.signer)
            .map_err(|error| DraftError::new(DraftErrorKind::Validation, error.to_string()))?;
        let offered = candidate
            .digest()
            .map_err(|error| DraftError::new(DraftErrorKind::Validation, format!("{error}")))?;

        let head_path = self.head_path(attempt);
        if let Some(parent) = head_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                DraftError::new(
                    DraftErrorKind::Storage,
                    format!("could not create {}: {error}", parent.display()),
                )
            })?;
        }
        let _lock = ProcessFileLock::acquire_exclusive_ordered(
            &self.head_lock_path(&attempt.id),
            DEFAULT_LOCK_TIMEOUT,
            LockOrder::PublicationAuxiliary,
        )?;

        if let Some(existing) = self.read_head(&head_path)? {
            if existing != offered {
                // Not ordinary concurrency: journal serialization means two
                // legitimate actors cannot establish different candidates for
                // one attempt, so this counts an integrity failure.
                crate::support::telemetry::Counter::PublicationOutcomeConflicts.increment();
                return Ok(RecordOnce::Conflict { existing, offered });
            }
            // Matching digests are not enough on their own. The head is a
            // claim *about* an object; verifying the object it names is what
            // makes `ExistingSame` mean "the same fact is already recorded"
            // rather than "the same fact is already claimed".
            self.require_stored_matches(attempt, candidate, &offered)?;
            return Ok(RecordOnce::ExistingSame);
        }

        // The object first. If the process dies here the result is an
        // unreachable orphan, which the next call reuses and GC can collect.
        // The reverse order would leave a head pointing at nothing, which no
        // retry can repair.
        self.objects.put(&attempt.id.to_string(), candidate)?;

        let bytes = serde_json::to_vec_pretty(&offered)
            .map_err(|error| DraftError::new(DraftErrorKind::Storage, format!("{error}")))?;
        write_atomic(&head_path, &bytes)?;
        Ok(RecordOnce::Created)
    }

    /// Require the stored object to be exactly this candidate.
    fn require_stored_matches(
        &self,
        attempt: &PublicationAttemptRef,
        candidate: &PublicationOutcome,
        offered: &PublicationOutcomeDigest,
    ) -> DraftResult<()> {
        let stored = self.objects.get(&attempt.id.to_string())?.ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "attempt '{}' has an outcome head naming {offered} but no outcome object",
                    attempt.id
                ),
            )
        })?;
        if &stored != candidate {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "attempt '{}' has an outcome head naming {offered} but its stored object is \
                     not the candidate that digest describes",
                    attempt.id
                ),
            ));
        }
        Ok(())
    }

    fn read_head(
        &self,
        head_path: &std::path::Path,
    ) -> DraftResult<Option<PublicationOutcomeDigest>> {
        match std::fs::read(head_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| {
                DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!(
                        "the outcome head at {} is unreadable: {error}",
                        head_path.display()
                    ),
                )
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(DraftError::new(
                DraftErrorKind::Storage,
                format!("could not read {}: {error}", head_path.display()),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::identifier::NamespacedId;
    use draft_dcg_contract::ids::ActorId;
    use draft_dcg_contract::producer::ProducerIdentity;
    use draft_dcg_contract::publication::{PublicationAttemptDigest, PublicationOutcomeKind};
    use draft_dcg_contract::receipt::ReceiptSignerBinding;
    use draft_dcg_contract::value::Timestamp;
    use draft_dcg_contract::Digest;

    fn attempt() -> PublicationAttemptRef {
        PublicationAttemptRef {
            id: PublicationAttemptId::parse("pat_000000000001").unwrap(),
            digest: PublicationAttemptDigest::new(Digest::of_bytes(b"attempt")),
        }
    }

    fn signer() -> ReceiptSignerBinding {
        ReceiptSignerBinding::new(
            ActorId::parse("act_000000000001").unwrap(),
            "key-1",
            "ed25519",
        )
        .unwrap()
    }

    fn identity() -> PrimaryOutcomeIdentity {
        PrimaryOutcomeIdentity {
            attempt: attempt(),
            receipt: ReceiptId::parse("rcp_000000000001").unwrap(),
            signer: signer(),
        }
    }

    fn outcome(kind: PublicationOutcomeKind) -> PublicationOutcome {
        PublicationOutcome {
            attempt: attempt(),
            receipt_id: ReceiptId::parse("rcp_000000000001").unwrap(),
            receipt_signer: signer(),
            outcome: kind,
            concluded_at: Timestamp::from_unix_nanos(0),
            provenance: ProducerIdentity::new(
                NamespacedId::parse("draft.core/publication").unwrap(),
                "1",
            )
            .unwrap(),
        }
    }

    fn succeeded() -> PublicationOutcome {
        outcome(PublicationOutcomeKind::Succeeded {
            external_reference: "remote-1".into(),
        })
    }

    fn failed() -> PublicationOutcome {
        outcome(PublicationOutcomeKind::Failed {
            reason: "refused".into(),
        })
    }

    #[test]
    fn the_first_recording_creates_and_an_identical_retry_is_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let store = PublicationOutcomeStore::new(directory.path());

        assert_eq!(
            store.record_once(&identity(), &succeeded()).unwrap(),
            RecordOnce::Created
        );
        assert_eq!(
            store.record_once(&identity(), &succeeded()).unwrap(),
            RecordOnce::ExistingSame,
            "a retry after an uncertain crash must converge, not fail"
        );
        assert_eq!(
            store.primary_outcome(&attempt()).unwrap(),
            Some(succeeded())
        );
    }

    #[test]
    fn a_different_candidate_conflicts_rather_than_converging() {
        // Recording `Failed` over a committed `Succeeded` would discard a
        // claim about whether an external effect occurred. Serialization means
        // this cannot happen legitimately, so it is surfaced, never resolved.
        let directory = tempfile::tempdir().unwrap();
        let store = PublicationOutcomeStore::new(directory.path());
        store.record_once(&identity(), &succeeded()).unwrap();

        let result = store.record_once(&identity(), &failed()).unwrap();
        assert!(matches!(result, RecordOnce::Conflict { .. }));
        assert!(!result.is_committed());
        assert_eq!(
            store.primary_outcome(&attempt()).unwrap(),
            Some(succeeded()),
            "the committed outcome must survive the attempt to replace it"
        );
    }

    #[test]
    fn an_orphaned_object_is_reused_rather_than_duplicated() {
        // The crash window between the object and the head. The retry finds no
        // head, writes the identical object again — which the create-once
        // binding accepts as idempotent — and establishes the head.
        let directory = tempfile::tempdir().unwrap();
        let store = PublicationOutcomeStore::new(directory.path());
        store.record_once(&identity(), &succeeded()).unwrap();

        std::fs::remove_file(store.head_path(&attempt())).unwrap();
        assert_eq!(
            store.primary_outcome(&attempt()).unwrap(),
            None,
            "an orphaned object is not an outcome until a head names it"
        );

        assert_eq!(
            store.record_once(&identity(), &succeeded()).unwrap(),
            RecordOnce::Created
        );
        assert_eq!(
            store.primary_outcome(&attempt()).unwrap(),
            Some(succeeded())
        );
    }

    #[test]
    fn a_head_without_its_object_is_an_integrity_failure_not_an_absence() {
        // The window the write order makes unreachable. If it is ever seen, it
        // must not read as "no outcome" — that would let a second attempt
        // proceed against an external effect Draft has a record of.
        let directory = tempfile::tempdir().unwrap();
        let store = PublicationOutcomeStore::new(directory.path());
        store.record_once(&identity(), &succeeded()).unwrap();
        std::fs::remove_file(store.objects.payload_path(&attempt().id.to_string())).unwrap();

        let error = store.primary_outcome(&attempt()).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn an_outcome_filed_under_the_wrong_attempt_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let store = PublicationOutcomeStore::new(directory.path());
        let elsewhere = PrimaryOutcomeIdentity {
            attempt: PublicationAttemptRef {
                id: PublicationAttemptId::parse("pat_000000000002").unwrap(),
                digest: PublicationAttemptDigest::new(Digest::of_bytes(b"attempt")),
            },
            ..identity()
        };

        let error = store.record_once(&elsewhere, &succeeded()).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::Validation);
    }
}
