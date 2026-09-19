//! What state the project's workflow is in, and what may legally happen next.
//!
//! Every surface — CLI, draftd, Console web, Console TUI — asks these
//! questions, and each one that answered them for itself would be a second
//! opinion about authority. So the answers are computed once, here, from the
//! same durable records the engines act on.
//!
//! ```text
//! Evidence → Assessment → Gate → Decision → Promotion → Baseline → Publication
//!                                              │                        │
//!                                     changes what the         delivers it, and
//!                                     project accepts          changes nothing
//! ```
//!
//! # Why availability is computed here and not in a frontend
//!
//! "Can I promote?" is a question about decisions, gates, revisions, journals
//! and the accepted Baseline. A frontend that inferred it from a lifecycle
//! enum would be re-deriving authority from a shadow of it, and would drift
//! the first time a rule changed. So this returns the answer *and the reason*,
//! and a surface renders both without judging.
//!
//! An available action is still not a permission. It says the preconditions
//! this reader could see were met; the operation re-checks everything under
//! its own guards, and a race is resolved there. What availability prevents is
//! offering somebody a button that cannot possibly work.
//!
//! # Why operation state is a projection and not the journal
//!
//! Promotion and Publication have journals with states like `Prepared` and
//! `AttemptPrepared`. Those are protocol internals: they change as the
//! protocol is refined, and a UI built on them would break or, worse, keep
//! rendering a state that no longer means what it did.
//!
//! [`OperationState`] is the user-facing vocabulary, and every variant is
//! derived from what the engine's own records say. Nothing here can report a
//! state the engine is not in, because nothing here holds state of its own.
//!
//! # Why a publication failure never shows as a Baseline failure
//!
//! [`PublicationView`] carries the Baseline it delivers and the state of the
//! delivery, separately. A failed delivery leaves the Baseline exactly as
//! authoritative as it was — so a surface has both facts and cannot render the
//! failure as a rollback, because there is no field here that would let it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use draft_dcg_contract::baseline::{BaselineId, BaselineManifest};
use draft_dcg_contract::ids::{
    ChangePackId, ProjectId, PromotionId, PublicationId, RevisionPackId,
};
use draft_dcg_contract::provider::ProviderProvenanceRef;
use draft_dcg_contract::publication::{DeliverySemantics, PublicationOutcomeKind};

use crate::app::authorization::AuthorizationStores;
use crate::app::publish::{promotion_of, route_for_baseline};
use crate::dcg::baseline::{current_baseline, BaselineRecord, BaselineStore};
use crate::dcg::change_pack::{ChangePack, ChangePackLifecycle, ChangePackStore};
use crate::dcg::decision::{Decision, DecisionOutcome};
use crate::dcg::revision_pack::{RevisionPack, RevisionPackStore};
use crate::evidence::assessment::Assessment;
use crate::evidence::Evidence;
use crate::gate::GateEvaluation;
use crate::project::Workspace;
use crate::promotion::journal::PromotionJournalState;
use crate::promotion::protocol::PromotionStores;
use crate::publication::barrier::PublicationBookkeepingResult;
use crate::publication::delivery::{retry_permission, RetryPermission};
use crate::publication::dispatch::{bookkeeping, DispatchStores};
use crate::publication::store::PublicationStore;
use crate::support::error::DraftResult;

/// How far a durable, restartable operation has got.
///
/// The complete user-facing vocabulary for Promotion and Publication. Each
/// variant is derived from the engine's own records; none of them is a state
/// this module keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    /// Nothing has been started.
    Pending,
    /// Something must happen first. The reason says what.
    Blocked,
    /// Started and not yet concluded — either in flight now, or interrupted
    /// and awaiting the recovery a later call performs. Draft cannot tell
    /// those apart from durable state alone, and says so rather than guessing.
    Running,
    /// Concluded in part, with work still owed that a retry completes.
    Recovering,
    /// Finished, with its result durable.
    Completed,
    /// Finished without achieving its effect. Terminal for this attempt.
    Failed,
}

/// One action a surface may offer, and whether it may offer it.
///
/// Like every type below it, this is a projection a surface reads and nothing
/// stores. That is why none of them denies unknown fields: strictness on read
/// is what a persisted contract needs, and treating a transient view as one
/// would freeze a shape whose whole purpose is to grow with the surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionAvailability {
    /// The stable action name, e.g. `promote`.
    pub action: String,
    pub available: bool,
    /// Why not. Always present when unavailable, so no surface has to invent
    /// an explanation for a disabled control.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl ActionAvailability {
    fn yes(action: &str) -> Self {
        Self {
            action: action.to_string(),
            available: true,
            reason: None,
        }
    }

    fn no(action: &str, reason: impl Into<String>) -> Self {
        Self {
            action: action.to_string(),
            available: false,
            reason: Some(reason.into()),
        }
    }
}

/// The Baseline the project currently accepts, and what established it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineView {
    pub baseline: BaselineId,
    pub project: ProjectId,
    pub manifest: BaselineManifest,
    pub record: BaselineRecord,
    /// This Baseline back to the project's first, newest first.
    pub lineage: Vec<BaselineId>,
    /// What established each Resource's accepted state. Immutable.
    pub composition: BTreeMap<draft_dcg_contract::ids::ResourceId, ProviderProvenanceRef>,
    /// Whether a Publication of this Baseline could be routed right now.
    ///
    /// A separate question from composition: composition never changes, this
    /// moves whenever the binding does. An unroutable Baseline is still fully
    /// accepted — only delivery is blocked.
    pub routable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_refusal: Option<String>,
}

/// A ChangePack and the revisions sealed against it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangePackView {
    pub change_pack: ChangePackId,
    pub lifecycle: ChangePackLifecycle,
    pub current_definition: draft_dcg_contract::Digest,
    /// Newest first.
    pub revisions: Vec<RevisionPack>,
}

/// One gate evaluation, with the derived answers a reader needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateView {
    pub evaluation: GateEvaluation,
    pub satisfied: bool,
    /// Conditions that failed, by id.
    pub unsatisfied: Vec<String>,
    /// Conditions that a person excused rather than the check passing.
    ///
    /// Derived structurally: the evaluator records a waived condition as
    /// satisfied *with* the waiver named in its detail, and a genuinely
    /// satisfied condition carries no detail at all. Surfacing them
    /// separately is the point — "somebody allowed this" and "this passed"
    /// must never look the same.
    pub waived: Vec<String>,
}

impl GateView {
    fn of(evaluation: GateEvaluation) -> Self {
        let unsatisfied = evaluation
            .conditions
            .iter()
            .filter(|condition| !condition.satisfied)
            .map(|condition| condition.id.clone())
            .collect();
        let waived = evaluation
            .conditions
            .iter()
            .filter(|condition| condition.satisfied && condition.detail.is_some())
            .map(|condition| condition.id.clone())
            .collect();
        Self {
            satisfied: evaluation.is_satisfied(),
            unsatisfied,
            waived,
            evaluation,
        }
    }
}

/// Everything decided about one revision, and what may follow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizationView {
    pub change_pack: ChangePackId,
    pub revision_pack: RevisionPackId,
    pub evidence: Vec<Evidence>,
    pub assessments: Vec<Assessment>,
    /// Who examined this revision, and what they wrote.
    ///
    /// Separate from `decisions`: reading a revision and concluding something
    /// about it are different acts, and an approval with no review behind it
    /// is exactly what an audit wants to be able to see.
    pub reviews: Vec<crate::dcg::review::Review>,
    /// How this revision's work was explained, when an explanation was
    /// recorded.
    ///
    /// Its own field rather than folded into evidence: an explanation is not a
    /// finding, and a reviewer reading one is being shown what changed rather
    /// than being told anything passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<crate::evidence::representation::RevisionPackRepresentationBundle>,
    pub gates: Vec<GateView>,
    /// The decisions on record about this exact revision.
    ///
    /// Immutable, so several may exist and none replaces another.
    pub decisions: Vec<Decision>,
    /// The promotion this revision would produce onto the accepted Baseline,
    /// if one has been started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promotion: Option<PromotionView>,
    pub actions: Vec<ActionAvailability>,
}

impl AuthorizationView {
    /// The approving decision citing a satisfied gate, if there is one.
    pub fn approving_decision(&self) -> Option<&Decision> {
        self.decisions.iter().find(|decision| {
            matches!(decision.outcome, DecisionOutcome::Approved)
                && self
                    .gates
                    .iter()
                    .any(|gate| gate.satisfied && gate.evaluation.covers(&decision.revision_pack))
        })
    }
}

/// A promotion, projected from its journal and record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionView {
    pub promotion: PromotionId,
    pub change_pack: ChangePackId,
    pub revision_pack: RevisionPackId,
    pub state: OperationState,
    /// The Baseline it accepted. Absent until the commit point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<BaselineId>,
    /// What the state means, in one sentence.
    pub detail: String,
}

/// One publication attempt's conclusion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicationView {
    pub publication: PublicationId,
    /// The Baseline being delivered. Unaffected by anything below.
    pub baseline: BaselineId,
    pub promotion: PromotionId,
    pub purpose: String,
    pub semantics: DeliverySemantics,
    pub state: OperationState,
    /// The outcome the engine recorded, where one is recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<PublicationOutcomeKind>,
    /// Whether another attempt may be made on the delivery semantics alone.
    ///
    /// `false` does not mean "never again" — it means a new attempt needs
    /// somebody to accept that it might duplicate a real-world effect.
    pub may_retry_automatically: bool,
    pub detail: String,
}

/// The project's whole workflow state, in one answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectWorkflowView {
    pub project: ProjectId,
    /// The authoritative project state. There is no other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<BaselineView>,
    pub change_packs: Vec<ChangePackView>,
    pub publications: Vec<PublicationView>,
    pub actions: Vec<ActionAvailability>,
    /// The one thing a person would do next, when there is an obvious one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_action: Option<String>,
}

/// The Baseline the project accepts, with its lineage and composition.
pub fn baseline_view(workspace: &Workspace) -> DraftResult<Option<BaselineView>> {
    let Some(baseline) = current_baseline(&workspace.layout)? else {
        return Ok(None);
    };
    let store = BaselineStore::new(workspace.layout.baselines_dir());
    let (Some(manifest), Some(record)) = (store.manifest(&baseline)?, store.record(&baseline)?)
    else {
        return Ok(None);
    };
    let composition = store
        .composition(&baseline)?
        .map(|value| value.resource_provenance)
        .unwrap_or_default();

    // Asked, not assumed. A Baseline established by a binding that has since
    // been unbound is still accepted; it simply has nowhere to be delivered.
    let (routable, route_refusal) = match route_for_baseline(workspace, &baseline) {
        Ok(_) => (true, None),
        Err(error) => (false, Some(error.message.clone())),
    };

    Ok(Some(BaselineView {
        project: manifest.project.clone(),
        lineage: store.lineage(&baseline)?,
        baseline,
        manifest,
        record,
        composition,
        routable,
        route_refusal,
    }))
}

/// Every ChangePack, with the revisions sealed against it.
pub fn change_pack_views(workspace: &Workspace) -> DraftResult<Vec<ChangePackView>> {
    let changes = ChangePackStore::new(workspace.layout.change_packs_dir()).list()?;
    let revisions = RevisionPackStore::new(workspace.layout.revision_packs_dir()).list()?;

    Ok(changes
        .into_iter()
        .map(|change: ChangePack| {
            let mut mine: Vec<RevisionPack> = revisions
                .iter()
                .filter(|revision| revision.change_pack == change.id)
                .cloned()
                .collect();
            mine.sort_by_key(|revision| std::cmp::Reverse(revision.sealed_at.as_unix_nanos()));
            ChangePackView {
                change_pack: change.id,
                lifecycle: change.lifecycle,
                current_definition: change.current_definition,
                revisions: mine,
            }
        })
        .collect())
}

/// Everything decided about one revision, and what may legally follow.
pub fn authorization_view(
    workspace: &Workspace,
    change: &ChangePackId,
    revision: &RevisionPackId,
) -> DraftResult<AuthorizationView> {
    let stores = AuthorizationStores::for_layout(&workspace.layout);

    let evidence: Vec<Evidence> = stores
        .evidence
        .list()?
        .into_iter()
        .filter(|value| value.covers(revision))
        .collect();
    let assessments: Vec<Assessment> = stores
        .assessments
        .list()?
        .into_iter()
        .filter(|value| value.covers(revision))
        .collect();
    let reviews: Vec<crate::dcg::review::Review> = stores
        .reviews
        .list()?
        .into_iter()
        .filter(|value| value.covers(revision))
        .collect();
    let gates: Vec<GateView> = stores
        .gates
        .list()?
        .into_iter()
        .filter(|value| value.covers(revision))
        .map(GateView::of)
        .collect();
    let decisions: Vec<Decision> = stores
        .decisions
        .list()?
        .into_iter()
        .filter(|value| value.covers(revision))
        .collect();

    let accepted = current_baseline(&workspace.layout)?;
    let promotion_id = crate::app::promotion::promotion_id_for(revision, accepted.as_ref())?;
    let promotion = promotion_view(workspace, &promotion_id)?;

    let mut view = AuthorizationView {
        change_pack: change.clone(),
        revision_pack: revision.clone(),
        evidence,
        assessments,
        reviews,
        representation: crate::evidence::representation::RepresentationStore::new(
            workspace.layout.representations_dir(),
        )
        .get(revision)?,
        gates,
        decisions,
        promotion,
        actions: Vec::new(),
    };
    view.actions = revision_actions(workspace, &view)?;
    Ok(view)
}

/// What a caller may legally do with this revision right now.
fn revision_actions(
    workspace: &Workspace,
    view: &AuthorizationView,
) -> DraftResult<Vec<ActionAvailability>> {
    let mut actions = Vec::new();

    // Assessing is always legal: judging work is not an authorization, and
    // refusing to let somebody record a judgement would be refusing the input
    // every later step needs.
    actions.push(ActionAvailability::yes("assess"));

    actions.push(if view.assessments.is_empty() {
        ActionAvailability::no(
            "evaluate_gate",
            "no assessment covers this revision yet; a gate over nothing would pass vacuously",
        )
    } else {
        ActionAvailability::yes("evaluate_gate")
    });

    let satisfied_gate = view.gates.iter().find(|gate| gate.satisfied);
    actions.push(match satisfied_gate {
        Some(_) => ActionAvailability::yes("decide"),
        None if view.gates.is_empty() => ActionAvailability::no(
            "decide",
            "no gate has been evaluated for this revision; an approval must be made with the \
             facts, not instead of them",
        ),
        None => ActionAvailability::no(
            "decide",
            format!(
                "the gate is not satisfied ({})",
                view.gates
                    .iter()
                    .flat_map(|gate| gate.unsatisfied.iter().cloned())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
    });

    actions.push(promotion_availability(workspace, view)?);
    Ok(actions)
}

fn promotion_availability(
    workspace: &Workspace,
    view: &AuthorizationView,
) -> DraftResult<ActionAvailability> {
    // A promotion already concluded is not a promotion available: offering it
    // again would invite a caller to promote the same work twice.
    if let Some(promotion) = &view.promotion {
        if promotion.state == OperationState::Completed {
            return Ok(ActionAvailability::no(
                "promote",
                format!(
                    "this revision was already promoted as {} into baseline {}",
                    promotion.promotion,
                    promotion
                        .baseline
                        .as_ref()
                        .map_or_else(|| "an accepted baseline".to_string(), ToString::to_string)
                ),
            ));
        }
        if matches!(
            promotion.state,
            OperationState::Running | OperationState::Recovering
        ) {
            return Ok(ActionAvailability::yes("promote"));
        }
    }

    if view.approving_decision().is_none() {
        return Ok(ActionAvailability::no(
            "promote",
            "no approving decision cites a satisfied gate over this exact revision",
        ));
    }

    // The ChangePack must still be open. A completed ChangePack's work is already in
    // an accepted Baseline, and revising it into a second one would accept the
    // same work twice.
    let change = ChangePackStore::new(workspace.layout.change_packs_dir())
        .read_unlocked(&view.change_pack)?;
    match change {
        Some(change) if change.lifecycle.accepts_work() => Ok(ActionAvailability::yes("promote")),
        Some(change) => Ok(ActionAvailability::no(
            "promote",
            format!(
                "ChangePack {} is {:?}, so it accepts no further work",
                change.id, change.lifecycle
            ),
        )),
        None => Ok(ActionAvailability::no(
            "promote",
            format!("ChangePack {} does not exist", view.change_pack),
        )),
    }
}

/// A promotion, projected from what its own records say.
///
/// Never from the journal mark alone: the mark describes what happened and is
/// written after the fact, so the record — which exists only once the
/// promotion finished everything it owed — is what distinguishes finished from
/// nearly finished.
pub fn promotion_view(
    workspace: &Workspace,
    promotion: &PromotionId,
) -> DraftResult<Option<PromotionView>> {
    let stores = PromotionStores::for_layout(&workspace.layout);
    let Some(record) = stores.journals.read_unlocked(promotion)? else {
        return Ok(None);
    };
    let journal = record.journal;
    let finished = stores.records.get(promotion)?;

    let (state, detail, baseline) = match journal.state {
        PromotionJournalState::Prepared => (
            OperationState::Running,
            "the promotion is durable but has not committed a baseline".to_string(),
            None,
        ),
        PromotionJournalState::Committed => (
            OperationState::Recovering,
            "the baseline was accepted; the promotion still owes its receipt and events"
                .to_string(),
            Some(journal.baseline.clone()),
        ),
        PromotionJournalState::Finalized => (
            OperationState::Completed,
            "the baseline was accepted and everything the promotion owed is recorded".to_string(),
            Some(
                finished
                    .map(|value| value.baseline)
                    .unwrap_or_else(|| journal.baseline.clone()),
            ),
        ),
    };

    Ok(Some(PromotionView {
        promotion: promotion.clone(),
        change_pack: journal.change_pack,
        revision_pack: journal.revision_pack,
        state,
        baseline,
        detail,
    }))
}

/// Every Publication this project has requested, with the engine's own view of
/// where each one is.
pub fn publication_views(workspace: &Workspace) -> DraftResult<Vec<PublicationView>> {
    let store = PublicationStore::for_layout(&workspace.layout);
    let dispatch = DispatchStores::for_layout(&workspace.layout);
    let mut views = Vec::new();

    for id in store.list()? {
        let Some(publication) = store.get(&id)? else {
            continue;
        };
        // Asked of the engine, through the same barrier a dispatch would take.
        // Nothing here decides what state the Publication is in.
        let barrier = bookkeeping(&dispatch, &id, publication.delivery_semantics)?;
        let outcome = latest_outcome(&dispatch, &id)?;

        let (state, detail) = match (&barrier, &outcome) {
            (PublicationBookkeepingResult::Clean, None) => (
                OperationState::Pending,
                "nothing has been delivered for this publication".to_string(),
            ),
            (
                PublicationBookkeepingResult::Clean,
                Some(PublicationOutcomeKind::Succeeded { external_reference }),
            ) => (
                OperationState::Completed,
                format!("delivered as {external_reference}"),
            ),
            (
                PublicationBookkeepingResult::Clean,
                Some(PublicationOutcomeKind::Failed { reason }),
            ) => (
                OperationState::Failed,
                format!("the target refused the delivery: {reason}"),
            ),
            (
                PublicationBookkeepingResult::Clean,
                Some(PublicationOutcomeKind::NoEffect { evidence }),
            ) => (
                OperationState::Failed,
                format!("the delivery provably did not happen: {evidence}"),
            ),
            (
                PublicationBookkeepingResult::Clean,
                Some(PublicationOutcomeKind::Indeterminate { reason }),
            ) => (
                OperationState::Blocked,
                format!(
                    "draft cannot establish whether the delivery happened: {reason}; another \
                     attempt needs somebody to accept that it may duplicate the effect"
                ),
            ),
            (PublicationBookkeepingResult::RecoverAllocatedAttempt { attempt }, _) => (
                OperationState::Recovering,
                format!("attempt {attempt} was interrupted and must be resumed before another"),
            ),
            (PublicationBookkeepingResult::PendingExternalResolution { attempt }, _) => (
                OperationState::Recovering,
                format!(
                    "attempt {attempt} may have caused an effect only the target can confirm; \
                     nothing local is blocked"
                ),
            ),
            (PublicationBookkeepingResult::Inconsistent { detail }, _) => (
                OperationState::Blocked,
                format!("the publication's local records contradict each other: {detail}"),
            ),
        };

        views.push(PublicationView {
            publication: id,
            baseline: publication.baseline.clone(),
            promotion: publication.promotion.clone(),
            purpose: publication.purpose.to_string(),
            semantics: publication.delivery_semantics,
            state,
            outcome,
            may_retry_automatically: retry_permission(publication.delivery_semantics)
                == RetryPermission::SafeToRetry,
            detail,
        });
    }
    Ok(views)
}

/// The recorded outcome of this Publication's in-flight or last attempt.
fn latest_outcome(
    stores: &DispatchStores,
    publication: &PublicationId,
) -> DraftResult<Option<PublicationOutcomeKind>> {
    // A Publication with no control record has never been dispatched against,
    // so there is nothing to have concluded.
    if stores.control.read_unlocked(publication)?.is_none() {
        return Ok(None);
    }
    // Every attempt that concluded left a journal naming its outcome.
    let mut latest = None;
    for attempt in stores.journals.list()? {
        let Some(record) = stores.journals.read_unlocked(&attempt)? else {
            continue;
        };
        if &record.publication != publication {
            continue;
        }
        if let Some(reference) = record.state.concluded_attempt() {
            if let Some(outcome) = stores.outcomes.primary_outcome(reference)? {
                latest = Some(outcome.outcome);
            }
        }
    }
    Ok(latest)
}

/// The project's whole workflow state.
pub fn project_view(workspace: &Workspace) -> DraftResult<ProjectWorkflowView> {
    let baseline = baseline_view(workspace)?;
    let changes = change_pack_views(workspace)?;
    let publications = publication_views(workspace)?;

    let actions = vec![
        match &baseline {
            Some(_) => ActionAvailability::yes("observe"),
            None => ActionAvailability::no(
                "observe",
                "this project accepts no baseline yet; initialize it first",
            ),
        },
        publish_availability(workspace, baseline.as_ref())?,
    ];

    let next_action = next_action(&baseline, &changes, &publications);
    Ok(ProjectWorkflowView {
        project: baseline
            .as_ref()
            .map(|view| view.project.clone())
            .unwrap_or_else(|| workspace.workspace_id.clone()),
        baseline,
        change_packs: changes,
        publications,
        actions,
        next_action,
    })
}

fn publish_availability(
    workspace: &Workspace,
    baseline: Option<&BaselineView>,
) -> DraftResult<ActionAvailability> {
    let Some(baseline) = baseline else {
        return Ok(ActionAvailability::no(
            "publish",
            "this project accepts no baseline, so there is nothing to publish",
        ));
    };
    // Publication delivers what promotion accepted. The project's first
    // Baseline was not promoted — nothing decided work into it — so there is
    // no authorization to deliver it anywhere.
    if promotion_of(workspace, &baseline.baseline)?.is_none() {
        return Ok(ActionAvailability::no(
            "publish",
            "the accepted baseline was never promoted, so nothing authorized delivering it",
        ));
    }
    if !baseline.routable {
        return Ok(ActionAvailability::no(
            "publish",
            baseline
                .route_refusal
                .clone()
                .unwrap_or_else(|| "the accepted baseline has no route".to_string()),
        ));
    }
    Ok(ActionAvailability::yes("publish"))
}

/// The one thing a person would obviously do next, if there is one.
///
/// Deliberately conservative: it names a step only when the project's state
/// makes it unambiguous. Guessing here would push people through a workflow
/// they did not choose.
fn next_action(
    baseline: &Option<BaselineView>,
    changes: &[ChangePackView],
    publications: &[PublicationView],
) -> Option<String> {
    if baseline.is_none() {
        return Some("init".to_string());
    }
    if let Some(publication) = publications
        .iter()
        .find(|view| view.state == OperationState::Recovering)
    {
        return Some(format!("publish --resume {}", publication.publication));
    }
    let open = changes
        .iter()
        .find(|view| view.lifecycle == ChangePackLifecycle::Active)?;
    Some(match open.revisions.first() {
        Some(revision) => format!("gate {}", revision.id),
        None => format!("seal {}", open.change_pack),
    })
}
