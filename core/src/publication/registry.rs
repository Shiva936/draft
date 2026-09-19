//! Publication creation — one request key, one Publication, forever.
//!
//! # Why creation needs a journal at all
//!
//! Creating a Publication writes three things that must agree: the immutable
//! object, the `request_key → PublicationRef` mapping that makes it findable,
//! and the `PublicationRequested` fact. A crash between them leaves a
//! question, and the wrong answer is expensive in both directions — a
//! duplicate Publication is a second authorization to affect the outside
//! world, and a lost one silently drops a request the user made.
//!
//! # Why the mapping is to a reference, not an id
//!
//! `PublicationRequestKey → PublicationRef` carries the **digest** as well as
//! the id. Mapping to a bare id would let the bytes under `pub_A` change while
//! the mapping still resolved, which is exactly how a request for one route
//! could come to resolve to another.
//!
//! So a stored object that no longer recomputes to the mapped digest is
//! [`CreationOutcome::Corrupt`] — never quietly reconstructed from the
//! journal's copy. The journal's payload is used only along the frozen commit
//! path; history is not rewritten to make a mismatch go away.
//!
//! # Why "the key maps elsewhere" is convergence, not an error
//!
//! If the key already resolves to a *different* Publication, this candidate
//! never became authoritative — somebody else's did. The caller converges on
//! the existing one, and emits no event: the request it made is satisfied by a
//! Publication that already exists.

use serde::{Deserialize, Serialize};

use draft_dcg_contract::publication::{PublicationDigest, PublicationRef};

/// How far a creation transaction got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreationJournalState {
    /// The candidate and its fact are durable; nothing is findable yet.
    Prepared,
    /// The mapping committed. The Publication is authoritative.
    Committed,
    /// The fact is drained and the transaction is history.
    Finalized,
    /// The candidate never became authoritative and was withdrawn.
    Abandoned,
}

/// What the registry holds for one request key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappedPublication<'a> {
    /// No mapping exists.
    Absent,
    /// The key maps to this reference.
    Present(&'a PublicationRef),
}

/// What creation concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreationOutcome {
    /// This transaction established the Publication. Drain its fact exactly
    /// once.
    Created(PublicationRef),
    /// The key already resolves to an existing Publication. The caller
    /// converges on it, and emits **no** event — the request is satisfied.
    Existing(PublicationRef),
    /// The mapping names a Publication whose stored bytes no longer match the
    /// digest it was mapped to.
    Corrupt { detail: &'static str },
}

/// Classify a creation transaction against what the registry now holds.
///
/// `stored_digest` is what the mapped object actually recomputes to, or `None`
/// if the object could not be loaded — a distinction the caller must make by
/// reading, never by assuming.
pub fn classify_creation(
    journal: CreationJournalState,
    mapping: MappedPublication<'_>,
    candidate: &PublicationRef,
    stored_digest: Option<&PublicationDigest>,
) -> CreationOutcome {
    match mapping {
        // Nothing is findable, so nothing was authorized. Whether the object
        // was written does not matter: an orphan without a mapping is
        // collectable, and no event is owed.
        MappedPublication::Absent => match journal {
            CreationJournalState::Committed => CreationOutcome::Corrupt {
                detail: "a creation transaction marked committed with no registry mapping, though \
                         the mapping commits before that mark",
            },
            _ => CreationOutcome::Created(candidate.clone()),
        },

        MappedPublication::Present(existing) if existing == candidate => match stored_digest {
            Some(digest) if digest == &candidate.digest => {
                CreationOutcome::Created(candidate.clone())
            }
            Some(_) => CreationOutcome::Corrupt {
                detail: "the registry maps this key to a Publication whose stored bytes do not \
                             recompute to the mapped digest",
            },
            None => CreationOutcome::Corrupt {
                detail: "the registry maps this key to a Publication whose object cannot be \
                             loaded",
            },
        },

        // Somebody else's candidate won. Converge; emit nothing.
        MappedPublication::Present(existing) => CreationOutcome::Existing(existing.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::ids::PublicationId;
    use draft_dcg_contract::Digest;

    fn reference(id: &str, bytes: &[u8]) -> PublicationRef {
        PublicationRef {
            id: PublicationId::parse(id).unwrap(),
            digest: PublicationDigest::new(Digest::of_bytes(bytes)),
        }
    }

    fn candidate() -> PublicationRef {
        reference("pub_00000000000a", b"candidate")
    }

    #[test]
    fn no_mapping_means_the_transaction_never_became_authoritative() {
        // The retry re-runs from the top and establishes the mapping. No event
        // is owed for the attempt that did not commit.
        assert_eq!(
            classify_creation(
                CreationJournalState::Prepared,
                MappedPublication::Absent,
                &candidate(),
                None
            ),
            CreationOutcome::Created(candidate())
        );
    }

    #[test]
    fn a_committed_mark_with_no_mapping_contradicts_the_write_order() {
        assert!(matches!(
            classify_creation(
                CreationJournalState::Committed,
                MappedPublication::Absent,
                &candidate(),
                None
            ),
            CreationOutcome::Corrupt { .. }
        ));
    }

    #[test]
    fn a_verifying_mapping_finalizes_and_drains_exactly_once() {
        assert_eq!(
            classify_creation(
                CreationJournalState::Committed,
                MappedPublication::Present(&candidate()),
                &candidate(),
                Some(&candidate().digest)
            ),
            CreationOutcome::Created(candidate())
        );
    }

    #[test]
    fn bytes_that_no_longer_match_the_mapped_digest_are_corrupt_not_reconstructed() {
        // The journal still holds a copy of the candidate. Using it to
        // "repair" the mapping would rewrite history to hide a substitution.
        let moved = PublicationDigest::new(Digest::of_bytes(b"something-else"));
        assert!(matches!(
            classify_creation(
                CreationJournalState::Committed,
                MappedPublication::Present(&candidate()),
                &candidate(),
                Some(&moved)
            ),
            CreationOutcome::Corrupt { .. }
        ));
        // And an object that will not load at all is equally not an absence.
        assert!(matches!(
            classify_creation(
                CreationJournalState::Committed,
                MappedPublication::Present(&candidate()),
                &candidate(),
                None
            ),
            CreationOutcome::Corrupt { .. }
        ));
    }

    #[test]
    fn a_key_that_maps_elsewhere_converges_without_a_second_event() {
        // Two callers requested the same Publication; one won. The loser
        // converges on the winner rather than creating a duplicate
        // authorization to affect the outside world.
        let winner = reference("pub_00000000000b", b"winner");
        assert_eq!(
            classify_creation(
                CreationJournalState::Prepared,
                MappedPublication::Present(&winner),
                &candidate(),
                Some(&winner.digest)
            ),
            CreationOutcome::Existing(winner)
        );
    }

    #[test]
    fn one_request_key_maps_to_one_reference_including_its_digest() {
        // Same id, different bytes: not the same Publication. A mapping to a
        // bare id would have accepted this.
        let same_id_new_bytes = reference("pub_00000000000a", b"retargeted");
        assert_eq!(
            classify_creation(
                CreationJournalState::Committed,
                MappedPublication::Present(&same_id_new_bytes),
                &candidate(),
                Some(&same_id_new_bytes.digest)
            ),
            CreationOutcome::Existing(same_id_new_bytes)
        );
    }
}
