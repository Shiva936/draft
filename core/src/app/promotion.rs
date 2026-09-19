//! Promotion — the only operation that advances the project's Baseline.
//!
//! Submitting work does not make it authoritative. Assessing it does not.
//! Passing a gate does not. Promotion is the single point at which a project
//! changes what it accepts, and it happens only on an approving Decision that
//! cites a satisfied gate over the exact revision being promoted.
//!
//! ```text
//! Evidence → Assessment → Gate → Decision → PROMOTION → Baseline
//!                                              ↓
//!                                        Publication (optional, separate)
//! ```
//!
//! # Why the parent Baseline is checked explicitly
//!
//! Two promotions can be authorized concurrently against the same accepted
//! state. If neither checked, the second would build its Baseline on a parent
//! that had already been superseded — recording lineage that never existed and
//! silently discarding the first promotion's acceptance.
//!
//! So a caller states the Baseline it believes is accepted, and a promotion
//! against a moved parent is refused rather than rebased. Rebasing would be a
//! decision about whether the two changes compose, which nobody made.
//!
//! # Why duplicate promotion converges instead of failing
//!
//! A promotion interrupted after its Baseline committed but before its record
//! finished must be safe to retry. The promotion id is derived from the
//! revision and the parent, so retrying the same promotion recomputes the same
//! id and finds the same record — while a *different* promotion of the same
//! revision onto the same parent is the same promotion by definition.
//!
//! What is refused is a conflicting one: the record store is create-once, so a
//! second promotion claiming the same id with a different Baseline is an
//! integrity failure rather than an overwrite.
//!
//! # Why publication is not here
//!
//! Publication delivers an accepted Baseline somewhere. It consumes what
//! promotion produced and can fail, be retried, or never happen at all,
//! without any of that changing what the project accepts. Coupling them would
//! make an unreachable external system able to invalidate accepted history.

use draft_dcg_contract::baseline::BaselineId;
use draft_dcg_contract::ids::{ChangePackId, DecisionId, PromotionId, RevisionPackId};

use draft_dcg_contract::receipt::ReceiptSignerBinding;
use draft_dcg_contract::Digest;

use crate::app::authorization::AuthorizationStores;
use crate::app::baseline;
use crate::dcg::baseline::{current_baseline, BaselineOrigin};
use crate::dcg::change_pack::{ChangePack, ChangePackLifecycle, ChangePackStore};
use crate::dcg::decision::Decision;
use crate::gate::GateEvaluation;
use crate::project::Workspace;
use crate::promotion::journal::{ChangePackMatch, PromotionJournalState};
use crate::promotion::protocol::{execute, PromotionEffects, PromotionProgress, PromotionStores};
use crate::promotion::record::PromotionJournal;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// What a caller asks promotion to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotionRequest {
    pub change_pack: ChangePackId,
    /// The exact revision being promoted.
    pub revision_pack: RevisionPackId,
    /// The approving Decision that authorizes it.
    pub decision: DecisionId,
    /// The gate that decision was made over.
    pub gate: String,
    /// The Baseline the caller believes is currently accepted.
    ///
    /// `None` asserts the project has never accepted one. Stated rather than
    /// read, so a promotion decided against state that has since moved is
    /// refused instead of silently rebased.
    pub expected_parent: Option<BaselineId>,
}

/// What promotion did.
///
/// The complete set of ways a promotion can end, and the shape every surface
/// sees. Both variants mean the Baseline named here is accepted — the
/// difference is only whether this call is the one that made it so.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum PromotionOutcome {
    /// This call advanced the Baseline.
    Promoted {
        promotion: PromotionId,
        baseline: BaselineId,
    },
    /// This exact promotion had already happened. Returned rather than
    /// refused: a retry after an uncertain interruption must converge.
    AlreadyPromoted {
        promotion: PromotionId,
        baseline: BaselineId,
    },
}

impl PromotionOutcome {
    pub fn baseline(&self) -> &BaselineId {
        match self {
            Self::Promoted { baseline, .. } | Self::AlreadyPromoted { baseline, .. } => baseline,
        }
    }

    pub fn promotion(&self) -> &PromotionId {
        match self {
            Self::Promoted { promotion, .. } | Self::AlreadyPromoted { promotion, .. } => promotion,
        }
    }
}

/// Validate a promotion request and, if it holds, run the promotion protocol.
///
/// The application decides *whether* this promotion is authorized and what it
/// intends; the domain protocol owns *how* it executes durably. Everything
/// after `execute` — journal, barrier, restart classification, the commit
/// point, finalization — belongs to `promotion::protocol`, and nothing here
/// reimplements it.
pub fn promote(
    app: &crate::app::App,
    workspace: &Workspace,
    request: &PromotionRequest,
) -> DraftResult<PromotionOutcome> {
    let stores = AuthorizationStores::for_layout(&workspace.layout);

    let decision = require_approving_decision(&stores, request)?;
    let gate = require_satisfied_gate(&stores, request)?;
    require_same_revision(&decision, &gate, request)?;

    let promotion = promotion_id_for(&request.revision_pack, request.expected_parent.as_ref())?;
    let protocol = PromotionStores::for_layout(&workspace.layout);

    // A promotion already in flight is resumed from its own journal, which is
    // where "was this the same promotion?" is answered. Checked before the
    // parent, because a promotion that already committed has itself moved it.
    let resuming = protocol.journals.read_unlocked(&promotion)?.is_some();
    if !resuming {
        require_expected_parent(workspace, request)?;
    }

    let effects = ApplicationEffects { app, workspace };
    let intent = PromotionJournal {
        promotion: promotion.clone(),
        revision_pack: request.revision_pack.clone(),
        change_pack: request.change_pack.clone(),
        // Filled by the commit; the journal records what the promotion will
        // accept, and the protocol replaces it with what it did accept.
        baseline: request
            .expected_parent
            .clone()
            .unwrap_or_else(|| BaselineId::new(Digest::of_bytes(b"draft.core/pending-baseline"))),
        receipt: receipt_id_for(&promotion)?,
        signer: signer_for(workspace)?,
        prepared_at: crate::support::clock::Clock::now(&crate::support::clock::SystemClock),
        activity_event_ids: (0..FINALIZATION_EVENTS.len())
            .map(|index| event_id_for(&promotion, index))
            .collect::<DraftResult<Vec<_>>>()?,
        expected_control: effects.control_digest()?,
        // The planned control state differs from the expected one by exactly
        // the Baseline this promotion accepts. Recovery compares whole values,
        // so a planned state equal to the expected one would make "did it
        // commit?" unanswerable.
        planned_control: planned_control_digest(&promotion),
        planned_change: planned_completion_digest(workspace, &request.change_pack)?,
        state: PromotionJournalState::Prepared,
    };

    let progress = execute(&protocol, &intent, &effects)?;
    Ok(match progress {
        PromotionProgress::Promoted {
            promotion,
            baseline,
        } => PromotionOutcome::Promoted {
            promotion,
            baseline,
        },
        PromotionProgress::AlreadyFinalized {
            promotion,
            baseline,
        }
        | PromotionProgress::ResumedAndFinalized {
            promotion,
            baseline,
        } => PromotionOutcome::AlreadyPromoted {
            promotion,
            baseline,
        },
    })
}

/// The application's implementation of the protocol's durable steps.
///
/// Each method is one thing the protocol asks for at a point it chooses. The
/// application cannot reorder them, and the protocol cannot reach past them
/// into application state.
struct ApplicationEffects<'a> {
    app: &'a crate::app::App,
    workspace: &'a Workspace,
}

impl PromotionEffects for ApplicationEffects<'_> {
    fn control_digest(&self) -> DraftResult<Digest> {
        Ok(Digest::of_bytes(
            current_baseline(&self.workspace.layout)?
                .map_or_else(|| "none".to_string(), |baseline| baseline.to_string())
                .as_bytes(),
        ))
    }

    fn change_match(&self, journal: &PromotionJournal) -> DraftResult<ChangePackMatch> {
        // Compared by whole value against what the journal planned, never by
        // reading the lifecycle alone: recovery has to tell a ChangePack *this*
        // promotion completed from one something else completed, and only the
        // exact planned value distinguishes them.
        let store = change_pack_store(&self.workspace.layout);
        let Some(current) = store.read_unlocked(&journal.change_pack)? else {
            return Ok(ChangePackMatch::Neither);
        };
        if change_digest(&current)? == journal.planned_change {
            return Ok(ChangePackMatch::PlannedCompleted);
        }
        Ok(if current.lifecycle == ChangePackLifecycle::Active {
            ChangePackMatch::ExpectedActive
        } else {
            ChangePackMatch::Neither
        })
    }

    fn commit_baseline(&self, journal: &PromotionJournal) -> DraftResult<BaselineId> {
        let accepted = baseline::accept_current(
            self.app,
            self.workspace,
            BaselineOrigin::Promotion {
                promotion: journal.promotion.clone(),
                change_revision: journal.revision_pack.clone(),
            },
        )?;
        Ok(accepted.record.baseline_id)
    }

    fn complete_change(&self, journal: &PromotionJournal) -> DraftResult<()> {
        // Mandatory once the Baseline is accepted, not optional: leaving the
        // ChangePack open would let the same work be revised and promoted again,
        // accepting it twice.
        //
        // A ChangePack that has since been abandoned is a contradiction rather
        // than something to force — its work is in an accepted Baseline.
        change_pack_store(&self.workspace.layout).complete(&journal.change_pack)?;
        Ok(())
    }

    /// Issue the preallocated receipt and append the preallocated events.
    ///
    /// Both identities were chosen before the commit, so replaying this after
    /// a crash writes the same receipt under the same id and appends the same
    /// events — converging rather than accumulating a second attestation of
    /// one promotion.
    fn finalize(&self, journal: &PromotionJournal, baseline: &BaselineId) -> DraftResult<()> {
        let issued_at = journal.prepared_at;
        crate::receipt::issue_and_store(
            &self.workspace.layout,
            draft_dcg_contract::ReceiptPayload {
                receipt_id: journal.receipt.clone(),
                subject: draft_dcg_contract::receipt::ReceiptKind::Promotion {
                    promotion: journal.promotion.clone(),
                    baseline: baseline.clone(),
                },
                issued_by: journal.signer.signer_identity.clone(),
                issued_at,
            },
            journal.signer.clone(),
            &signing_key(self.workspace)?,
        )?;

        let ledger = activity_log(self.workspace);
        let actor = journal.signer.signer_identity.to_string();
        let metadata = serde_json::json!({
            "promotion": journal.promotion.to_string(),
            "revision_pack_id": journal.revision_pack.to_string(),
            "change_pack_id": journal.change_pack.to_string(),
            "baseline": baseline.to_string(),
            "receipt": journal.receipt.to_string(),
        });

        // One preallocated id per event, in the order the journal reserved
        // them. A shortfall is a journal that promised fewer events than
        // finalization emits, which would leave part of this promotion
        // unrecorded — so it is refused rather than papered over with a
        // freshly minted id.
        for (event_id, kind) in journal.activity_event_ids.iter().zip(FINALIZATION_EVENTS) {
            crate::app::activity::record_with_id(
                &ledger,
                event_id,
                &crate::app::activity::DomainAuditFact::new(
                    *kind,
                    actor.clone(),
                    journal.prepared_at,
                )
                .about(journal.change_pack.to_string())
                .with(metadata.clone()),
            )?;
        }
        if journal.activity_event_ids.len() < FINALIZATION_EVENTS.len() {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "this promotion preallocated {} event ids but finalization records {}; the \
                     unrecorded events would be lost to any later reader",
                    journal.activity_event_ids.len(),
                    FINALIZATION_EVENTS.len()
                ),
            ));
        }
        Ok(())
    }
}

/// What finalizing one promotion appends, in order.
///
/// Named as a list rather than emitted ad hoc so the preallocation and the
/// append can be checked against each other: the journal reserves exactly this
/// many ids, and recovery replays exactly these events.
const FINALIZATION_EVENTS: &[crate::activity::EventKind] = &[
    crate::activity::EventKind::PromotionCommitted,
    crate::activity::EventKind::BaselinePromoted,
    crate::activity::EventKind::ChangePackCompleted,
    crate::activity::EventKind::ReceiptIssued,
    crate::activity::EventKind::PromotionFinalized,
];

/// The project's Activity ledger.
fn activity_log(workspace: &Workspace) -> crate::activity::ActivityLog {
    crate::activity::ActivityLog::new(
        workspace.layout.events_dir(),
        workspace.workspace_id.to_string(),
    )
}

/// The key this project's receipts are signed with.
fn signing_key(workspace: &Workspace) -> DraftResult<crate::trust::signing::Keypair> {
    let home = crate::project::home::DraftGlobalStore::locate()?;
    let (_actor, keypair) = crate::trust::identity::global::active_signer(&home)?;
    let _ = workspace;
    Ok(keypair)
}

/// Who will sign this promotion's receipt, named before it is signed.
///
/// The key id is the signing key's own stable identifier rather than a label
/// for the subsystem: a verifier resolving the binding has to arrive at the
/// exact public key the signature was made with, and "which part of Draft
/// asked" does not identify a key.
fn signer_for(workspace: &Workspace) -> DraftResult<ReceiptSignerBinding> {
    ReceiptSignerBinding::new(
        baseline::actor_id_of(&workspace.layout)?,
        signing_key(workspace)?.public_key_id(),
        "ed25519",
    )
    .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}

fn event_id_for(promotion: &PromotionId, index: usize) -> DraftResult<String> {
    let seed = Digest::of_bytes(format!("event|{promotion}|{index}").as_bytes());
    Ok(format!("evt_{}", short_hex(&seed)))
}

/// The control digest this promotion intends to produce.
///
/// Distinct from the expected one by construction: recovery classifies a crash
/// by comparing the current control state against these two, and equal values
/// would make the two outcomes indistinguishable.
fn planned_control_digest(promotion: &PromotionId) -> Digest {
    Digest::of_bytes(format!("planned-control|{promotion}").as_bytes())
}

/// The exact ChangePack value this promotion's completion will produce.
///
/// Computed from the ChangePack as it stands, so recovery can compare whole
/// values rather than inspecting a lifecycle field that says nothing about
/// *which* promotion completed it.
fn planned_completion_digest(workspace: &Workspace, change: &ChangePackId) -> DraftResult<Digest> {
    let store = change_pack_store(&workspace.layout);
    let current = store.read_unlocked(change)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::NotFound,
            format!("ChangePack '{change}' does not exist, so nothing can be promoted from it"),
        )
    })?;
    // A retried promotion re-derives the same planned value: the ChangePack it
    // already completed is what it planned to complete, so the digest has to
    // match rather than the projection refusing the transition again.
    if current.lifecycle == ChangePackLifecycle::Completed {
        return change_digest(&current);
    }
    change_digest(&current.transition(ChangePackLifecycle::Completed)?)
}

pub(crate) fn change_digest(change: &ChangePack) -> DraftResult<Digest> {
    Digest::parse(crate::support::hashing::try_canonical_hash(change)?)
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}

pub(crate) fn change_pack_store(layout: &crate::project::layout::DraftLayout) -> ChangePackStore {
    ChangePackStore::new(layout.change_packs_dir())
}

fn require_approving_decision(
    stores: &AuthorizationStores,
    request: &PromotionRequest,
) -> DraftResult<Decision> {
    let decision = stores.decisions.get(&request.decision)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::ReviewRequired,
            format!(
                "promotion cites decision '{}', which does not exist",
                request.decision
            ),
        )
    })?;
    decision.validate()?;

    if !decision.covers(&request.revision_pack) {
        return Err(DraftError::new(
            DraftErrorKind::StaleRevision,
            format!(
                "decision '{}' judged revision '{}' but promotion is for '{}'; a judgement does \
                 not carry across revisions",
                decision.id, decision.revision_pack, request.revision_pack
            ),
        ));
    }
    if !decision.is_approval() {
        return Err(DraftError::new(
            DraftErrorKind::ReviewRequired,
            format!(
                "decision '{}' does not approve revision '{}', so nothing authorizes promoting it",
                decision.id, request.revision_pack
            ),
        ));
    }
    Ok(decision)
}

fn require_satisfied_gate(
    stores: &AuthorizationStores,
    request: &PromotionRequest,
) -> DraftResult<GateEvaluation> {
    let gate = stores.gates.get(&request.gate)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::ReviewRequired,
            format!(
                "promotion cites gate '{}', which does not exist",
                request.gate
            ),
        )
    })?;
    gate.validate()?;
    if !gate.is_satisfied() {
        let unsatisfied: Vec<&str> = gate
            .unsatisfied()
            .into_iter()
            .map(|condition| condition.id.as_str())
            .collect();
        return Err(DraftError::new(
            DraftErrorKind::GateUnsatisfied,
            format!(
                "gate '{}' is not satisfied ({}), so revision '{}' may not be promoted",
                gate.id,
                unsatisfied.join(", "),
                request.revision_pack
            ),
        ));
    }
    Ok(gate)
}

/// Both the decision and the gate must speak about the revision being promoted.
///
/// Checked here as well as at creation because the two facts are stored
/// separately: a decision citing one revision and a gate citing another would
/// otherwise combine into an authorization neither of them gave.
fn require_same_revision(
    decision: &Decision,
    gate: &GateEvaluation,
    request: &PromotionRequest,
) -> DraftResult<()> {
    if !gate.covers(&request.revision_pack) {
        return Err(DraftError::new(
            DraftErrorKind::StaleRevision,
            format!(
                "gate '{}' evaluated revision '{}' but promotion is for '{}'",
                gate.id, gate.revision_pack, request.revision_pack
            ),
        ));
    }
    if decision.revision_pack != gate.revision_pack {
        return Err(DraftError::new(
            DraftErrorKind::StaleRevision,
            format!(
                "decision '{}' judged '{}' while gate '{}' evaluated '{}'; an authorization \
                 assembled from facts about different revisions is not an authorization",
                decision.id, decision.revision_pack, gate.id, gate.revision_pack
            ),
        ));
    }
    Ok(())
}

/// The accepted Baseline must be the one the decision was made against.
fn require_expected_parent(workspace: &Workspace, request: &PromotionRequest) -> DraftResult<()> {
    let current = current_baseline(&workspace.layout)?;
    if current == request.expected_parent {
        return Ok(());
    }
    Err(DraftError::new(
        DraftErrorKind::StaleBaseline,
        format!(
            "this promotion was decided against Baseline {}, but the project now accepts {}",
            describe(request.expected_parent.as_ref()),
            describe(current.as_ref())
        ),
    )
    .with_suggestion(
        "re-evaluate the gate and decision against the current Baseline; Draft will not rebase an \
         authorization onto state nobody judged it against",
    ))
}

fn describe(baseline: Option<&BaselineId>) -> String {
    baseline.map_or_else(|| "none".to_string(), ToString::to_string)
}

/// The promotion that accepts this revision onto this parent.
///
/// Derived, so retrying an interrupted promotion recomputes the same id and
/// converges on the record it already wrote.
pub fn promotion_id_for(
    revision: &RevisionPackId,
    parent: Option<&BaselineId>,
) -> DraftResult<PromotionId> {
    let seed =
        draft_dcg_contract::Digest::of_bytes(format!("{revision}|{}", describe(parent)).as_bytes());
    PromotionId::parse(format!("pro_{}", short_hex(&seed)))
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}

fn receipt_id_for(promotion: &PromotionId) -> DraftResult<draft_dcg_contract::ids::ReceiptId> {
    let seed = draft_dcg_contract::Digest::of_bytes(format!("receipt|{promotion}").as_bytes());
    draft_dcg_contract::ids::ReceiptId::parse(format!("rcp_{}", short_hex(&seed)))
        .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
}

fn short_hex(digest: &draft_dcg_contract::Digest) -> String {
    digest
        .as_str()
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .chars()
        .take(24)
        .collect()
}
