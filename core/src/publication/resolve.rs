//! Recording a later, better-informed interpretation of a recorded outcome.
//!
//! Draft sometimes has to write `Indeterminate`: the call did not complete in
//! a way that establishes whether the effect occurred. That is an honest
//! answer, and it is immutable — the primary outcome is never rewritten,
//! because rewriting it would make the record of what Draft actually knew at
//! the time unrecoverable.
//!
//! When the truth later arrives — the target's operator confirms it, or a
//! query answers — it becomes authoritative as a [`PublicationResolution`]
//! sitting *alongside* the outcome, advancing a head that can itself be
//! superseded.
//!
//! # Why this is not "fixing" the outcome
//!
//! An outcome says what Draft could establish at the moment it concluded. A
//! resolution says what somebody later established, on whose authority, and
//! when. Both are true, and a reader needs both: "we did not know, then we
//! learned" is a different history from "we knew all along", and only one of
//! them explains why a retry authorization was issued in between.
//!
//! # The two phases, and why they are in this order
//!
//! ```text
//! Phase R  recover any prior unresolved transaction   NO current authority
//! Phase C  create a new one                           CURRENT authority required
//! ```
//!
//! Finishing something that already committed is not a new decision. If Phase
//! R needed current authority, a grant revoked after a Resolution committed
//! would leave it permanently unfinalizable — its head advanced and its
//! bookkeeping stuck — because the authority that authorized it is gone.
//! Refusing to finish would not undo the commit; it would only leave it
//! unreadable.
//!
//! Phase C never skips current authority, for the mirror-image reason:
//! proposing something genuinely new is always a fresh decision.
//!
//! [`crate::publication::resolution`] owns both classifications; this drives
//! them against storage.

use std::path::PathBuf;

use draft_dcg_contract::ids::ActorId;
use draft_dcg_contract::publication::{
    PublicationOutcome, PublicationOutcomeDigest, PublicationResolution,
    PublicationResolutionDigest, PublicationResolutionKind,
};
use draft_dcg_contract::receipt::ReceiptSignerBinding;
use draft_dcg_contract::value::Timestamp;

use crate::publication::authority::DispatchAuthority;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::immutable_store::ImmutableFactStore;
use crate::support::lock_order::LockOrder;
use crate::support::process_lock::ProcessFileLock;
use crate::support::record_guard::DEFAULT_LOCK_TIMEOUT;

/// Resolutions, and the head naming the one in force for each outcome.
#[derive(Debug, Clone)]
pub struct ResolutionStore {
    objects: ImmutableFactStore<PublicationResolution>,
    heads: PathBuf,
}

impl ResolutionStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        let directory = directory.into();
        Self {
            objects: ImmutableFactStore::new(directory.join("objects")),
            heads: directory.join("heads"),
        }
    }

    pub fn for_layout(layout: &crate::project::layout::DraftLayout) -> Self {
        Self::new(layout.publication_resolutions_dir())
    }

    fn head_path(&self, outcome: &PublicationOutcomeDigest) -> PathBuf {
        self.heads.join(format!("{}.json", key(outcome)))
    }

    /// The stable sidecar an outcome's resolution head is locked on.
    ///
    /// A sidecar, never the head file: the head is replaced by atomic rename,
    /// so a lock on its inode would stop protecting anything the moment a
    /// write landed.
    fn head_lock_path(&self, outcome: &PublicationOutcomeDigest) -> PathBuf {
        self.heads.join(format!("{}.lock", key(outcome)))
    }

    /// Every resolution object this project holds, by its own digest.
    ///
    /// Objects, not heads. A superseded resolution stays immutable and its
    /// receipt stays a valid historical attestation, so a listing that showed
    /// only the head would hide the chain an audit reads.
    pub fn list(&self) -> DraftResult<Vec<String>> {
        self.objects.list_ids()
    }

    /// The resolution currently in force for an outcome, if any is.
    pub fn head(
        &self,
        outcome: &PublicationOutcomeDigest,
    ) -> DraftResult<Option<PublicationResolution>> {
        let path = self.head_path(outcome);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(DraftError::storage(format!(
                    "could not read the resolution head at {}: {error}",
                    path.display()
                )))
            }
        };
        let digest: String = serde_json::from_slice(&bytes).map_err(|error| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "the resolution head at {} is unreadable: {error}",
                    path.display()
                ),
            )
        })?;
        // A head naming an object that does not load is an integrity failure,
        // not an absence: "no resolution" and "a resolution we cannot read"
        // are different answers and must never collapse into one.
        self.objects
            .get(&digest)?
            .map(Ok)
            .unwrap_or_else(|| {
                Err(DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!("resolution head names {digest}, which is not stored"),
                ))
            })
            .map(Some)
    }

    fn write_head(
        &self,
        outcome: &PublicationOutcomeDigest,
        digest: &PublicationResolutionDigest,
    ) -> DraftResult<()> {
        let path = self.head_path(outcome);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                DraftError::storage(format!("could not create {}: {error}", parent.display()))
            })?;
        }
        let encoded = serde_json::to_vec(&key_of(digest))
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?;
        crate::support::fsutil::write_atomic(&path, &encoded)
    }
}

fn key(outcome: &PublicationOutcomeDigest) -> String {
    outcome.digest().to_string().replace(':', "_")
}

fn key_of(digest: &PublicationResolutionDigest) -> String {
    digest.digest().to_string().replace(':', "_")
}

/// Everything resolving an outcome needs told.
pub struct ResolveRequest<'a> {
    /// The outcome being interpreted.
    pub outcome: &'a PublicationOutcome,
    /// What is now known to have happened.
    pub resolution: PublicationResolutionKind,
    pub actor: ActorId,
    pub signer: ReceiptSignerBinding,
    pub receipt: draft_dcg_contract::ids::ReceiptId,
    /// How this was established, in the resolver's words.
    pub rationale: String,
    pub resolved_at: Timestamp,
}

/// Record a resolution, advancing the head for its outcome.
///
/// `authority` is Phase 1's result: a resolution rests on current authority,
/// because proposing an interpretation is a new decision however old the
/// outcome is.
///
/// The whole read-decide-write runs under the outcome's head lock (order 9),
/// so no two resolvers interleave inside it. Within that section the physical
/// order is frozen: object first, head second. A crash between them leaves an
/// unreachable orphan the next call reuses; the reverse order would leave a
/// head naming nothing, which no retry can repair.
pub fn resolve(
    store: &ResolutionStore,
    request: &ResolveRequest<'_>,
    authority: &DispatchAuthority,
) -> DraftResult<PublicationResolutionDigest> {
    let outcome_digest = request
        .outcome
        .digest()
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?;

    std::fs::create_dir_all(&store.heads).map_err(|error| {
        DraftError::storage(format!(
            "could not create {}: {error}",
            store.heads.display()
        ))
    })?;
    let _lock = ProcessFileLock::acquire_exclusive_ordered(
        &store.head_lock_path(&outcome_digest),
        DEFAULT_LOCK_TIMEOUT,
        LockOrder::PublicationAuxiliary,
    )?;

    // The head as it stands. Read inside the lock that will replace it, so
    // there is no window between deciding what to supersede and superseding
    // it — which is why this needs no compare-exchange and no journal.
    //
    // `resolution::classify_prior` and `creation_step` model the alternative:
    // a transaction that releases its guards between reading the head and
    // advancing it, and must then recover whatever appeared in between. This
    // implementation does not release, so those states are unreachable here.
    // Calling them anyway would be decoration — they could only ever return
    // one value — and decoration that looks like a check is worse than no
    // check at all.
    let current = store.head(&outcome_digest)?;

    let grant = authority
        .authority_decision
        .considered
        .iter()
        .next()
        .cloned()
        .ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                "a permitted decision cited no authority, so nothing authorizes the resolution",
            )
        })?;

    let superseded = match &current {
        Some(head) => Some(
            head.digest()
                .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?,
        ),
        None => None,
    };

    let resolution = PublicationResolution {
        outcome: outcome_digest.clone(),
        receipt_id: request.receipt.clone(),
        receipt_signer: request.signer.clone(),
        resolution: request.resolution.clone(),
        supersedes: superseded,
        actor: request.actor.clone(),
        authority: grant,
        authority_decision: authority.authority_decision.clone(),
        project_security_state_at_resolution: authority.project_security_state.clone(),
        policy_digest_at_resolution: authority.policy_digest.clone(),
        global_registry_revisions_at_resolution: authority.registry_revisions.clone(),
        rationale: request.rationale.clone(),
        resolved_at: request.resolved_at,
    };

    // A supersession chain may never cross outcomes: replacing outcome A's
    // interpretation with one written about outcome B would silently
    // re-attribute an external result.
    resolution
        .validate_advancing(&outcome_digest, current.as_ref())
        .map_err(|error| {
            crate::support::telemetry::Counter::PublicationResolutionHeadConflicts.increment();
            DraftError::new(DraftErrorKind::Validation, error.to_string())
        })?;

    let digest = resolution
        .digest()
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))?;

    // Object first, head second. A crash between them leaves an unreachable
    // orphan the next call rewrites identically; the reverse order would leave
    // a head naming nothing, which no retry can repair.
    store.objects.put(&key_of(&digest), &resolution)?;
    store.write_head(&outcome_digest, &digest)?;
    Ok(digest)
}
