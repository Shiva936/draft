//! A RecoveryPlan — what Doctor produces instead of guessing.
//!
//! Every restart table in Draft ends the same way: combinations no legal write
//! order produces are refused, never resolved automatically. That refusal is
//! correct, and on its own it leaves a person stuck with a project they cannot
//! move and an error message they cannot act on.
//!
//! A RecoveryPlan is the bridge. It records what was observed, what it
//! concludes, and what it proposes — as an immutable fact, reviewable before
//! anything is touched.
//!
//! # Why the plan binds the state it was made from
//!
//! A plan is a conclusion drawn from a specific moment. Between drawing it and
//! applying it, another actor may legitimately have resolved the problem, or
//! made it a different problem. Applying a stale plan would then write a repair
//! for a situation that no longer exists — which is worse than the original
//! inconsistency, because the original was at least honest.
//!
//! So the plan carries the exact digest of the state it read, and applying it
//! re-checks that digest first. A plan whose world moved is refused and must
//! be re-drawn, not forced.
//!
//! # Why authority is required and never inherited
//!
//! A plan's actions rewrite or discard durable state — that is what makes it
//! useful and what makes it dangerous. Whoever applies one is taking
//! responsibility for a judgement Draft explicitly declined to make, so the
//! authority is cited exactly, like every other authorization-bearing fact,
//! and a plan proposing a destructive action with no cited authority is
//! invalid rather than merely unauthorized.
//!
//! Being able to *see* the problem is not authority to *change* it: reading a
//! project and repairing one are different permissions.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use draft_dcg_contract::ids::{ActorId, RecoveryPlanId};
use draft_dcg_contract::security::SecurityFactRef;
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::Digest;

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::immutable_store::{ImmutableFactStore, StoreOutcome};

/// What a plan proposes doing.
///
/// Deliberately coarse. A fine-grained action vocabulary would invite plans
/// that read as ordinary operations, and the whole point is that these are
/// not — each one is a person overriding a refusal Draft made deliberately.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RecoveryAction {
    /// Record the inconsistency and change nothing.
    ///
    /// A real outcome, not a placeholder: some inconsistencies are best left
    /// documented and untouched, and a plan that says so is more useful than
    /// no plan.
    DocumentOnly { detail: String },
    /// Mark a journal terminal without performing its remaining work.
    ///
    /// Destructive: it asserts a conclusion about a transaction Draft could
    /// not classify.
    AbandonJournal {
        journal: String,
        justification: String,
    },
    /// Rebuild a derived artifact from its authoritative source.
    ///
    /// Non-destructive by construction — an index rebuilt from the log it
    /// indexes cannot lose anything the log still holds.
    RebuildDerived { artifact: String, source: String },
    /// Quarantine an artifact so it stops being read, without deleting it.
    ///
    /// Preferred over deletion wherever it will do: a quarantined artifact can
    /// still be examined afterwards, and a deleted one cannot.
    Quarantine { artifact: String, reason: String },
}

impl RecoveryAction {
    /// Whether this action changes state a reader would otherwise trust.
    ///
    /// Drives the authority requirement, so the two cannot drift apart:
    /// there is no way to add a destructive action that does not also demand
    /// authority.
    pub fn is_destructive(&self) -> bool {
        match self {
            Self::DocumentOnly { .. } | Self::RebuildDerived { .. } => false,
            Self::AbandonJournal { .. } | Self::Quarantine { .. } => true,
        }
    }
}

/// An immutable proposal for resolving one inconsistency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryPlan {
    pub id: RecoveryPlanId,
    /// What Draft refused to resolve, in the words of the check that refused.
    pub finding: String,
    /// The digest of the exact state this conclusion was drawn from.
    ///
    /// Re-checked before the plan is applied, so a plan whose world moved is
    /// refused rather than forced onto a situation it no longer describes.
    pub observed_state: Digest,
    /// What the plan proposes, in order.
    pub actions: Vec<RecoveryAction>,
    /// The exact security facts permitting the destructive actions.
    ///
    /// Exact refs, so a verifier can re-resolve each one and detect
    /// substitution — the same discipline every other authorization-bearing
    /// fact follows.
    pub authority: BTreeSet<SecurityFactRef>,
    pub drawn_by: ActorId,
    pub drawn_at: Timestamp,
}

impl RecoveryPlan {
    /// Check the plan is internally coherent.
    pub fn validate(&self) -> DraftResult<()> {
        if self.actions.is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "recovery plan '{}' proposes nothing; a plan that proposes nothing is a \
                     finding, and DocumentOnly is how a finding is recorded",
                    self.id
                ),
            ));
        }
        if self.finding.trim().is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "recovery plan '{}' names no finding, so nothing explains why its actions are \
                     warranted",
                    self.id
                ),
            ));
        }
        if self.is_destructive() && self.authority.is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "recovery plan '{}' proposes a destructive action but cites no authority; \
                     seeing a problem is not permission to change it",
                    self.id
                ),
            ));
        }
        Ok(())
    }

    /// Whether any proposed action is destructive.
    pub fn is_destructive(&self) -> bool {
        self.actions.iter().any(RecoveryAction::is_destructive)
    }

    /// Whether this plan may still be applied against `current_state`.
    ///
    /// The whole guard against a stale repair. A plan is a conclusion about a
    /// moment; if the moment has passed, the conclusion is no longer about
    /// anything.
    pub fn applies_to(&self, current_state: &Digest) -> bool {
        &self.observed_state == current_state
    }

    /// Check the plan may be applied now.
    pub fn authorize(&self, current_state: &Digest) -> DraftResult<()> {
        self.validate()?;
        if !self.applies_to(current_state) {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "recovery plan '{}' was drawn from state {} but the project is now at {}; \
                     re-run the diagnosis rather than applying a repair for a situation that has \
                     moved",
                    self.id, self.observed_state, current_state
                ),
            )
            .with_suggestion("Run `draft doctor` again to draw a plan from current state."));
        }
        Ok(())
    }
}

/// Recovery plans, written once and never revised.
///
/// A revised plan is a different plan: keeping the original is what lets
/// somebody afterwards see what was proposed at the time, including the
/// proposal that was rejected.
#[derive(Debug, Clone)]
pub struct RecoveryPlanStore {
    plans: ImmutableFactStore<RecoveryPlan>,
}

impl RecoveryPlanStore {
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            plans: ImmutableFactStore::new(directory),
        }
    }

    /// Record a plan. Validates before storing, so an incoherent plan never
    /// becomes a durable fact somebody can later act on.
    pub fn put(&self, plan: &RecoveryPlan) -> DraftResult<StoreOutcome> {
        plan.validate()?;
        self.plans.put(&plan.id.to_string(), plan)
    }

    pub fn get(&self, id: &RecoveryPlanId) -> DraftResult<Option<RecoveryPlan>> {
        self.plans.get(&id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::identifier::ScopedId;
    use draft_dcg_contract::security::SecurityControlKindId;

    fn grant() -> SecurityFactRef {
        SecurityFactRef::new(
            SecurityControlKindId::parse("draft.security/authority-grant.v1").unwrap(),
            Some(ScopedId::parse("auth_000000000001").unwrap()),
            Digest::of_bytes(b"grant"),
        )
    }

    fn plan(actions: Vec<RecoveryAction>, authority: BTreeSet<SecurityFactRef>) -> RecoveryPlan {
        RecoveryPlan {
            id: RecoveryPlanId::parse("rcv_000000000001").unwrap(),
            finding: "the journal is Dispatching but no outcome exists".into(),
            observed_state: Digest::of_bytes(b"state-1"),
            actions,
            authority,
            drawn_by: ActorId::parse("act_000000000001").unwrap(),
            drawn_at: Timestamp::from_unix_nanos(0),
        }
    }

    fn abandon() -> RecoveryAction {
        RecoveryAction::AbandonJournal {
            journal: "pat_a".into(),
            justification: "the external system confirmed nothing was received".into(),
        }
    }

    #[test]
    fn a_destructive_plan_without_authority_is_invalid_not_merely_unauthorized() {
        // The distinction matters: an unauthorized plan could be applied by
        // someone with more rights, an invalid one cannot be applied at all.
        let error = plan(vec![abandon()], BTreeSet::new())
            .validate()
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::Validation);

        plan(vec![abandon()], [grant()].into_iter().collect())
            .validate()
            .unwrap();
    }

    #[test]
    fn a_non_destructive_plan_needs_no_authority() {
        // Rebuilding an index from the log it indexes cannot lose anything the
        // log still holds, so requiring a grant would be ceremony.
        plan(
            vec![RecoveryAction::RebuildDerived {
                artifact: "events.index".into(),
                source: "events.log".into(),
            }],
            BTreeSet::new(),
        )
        .validate()
        .unwrap();
    }

    #[test]
    fn a_plan_drawn_from_a_state_that_has_moved_is_refused() {
        // Another actor legitimately resolved it, or made it a different
        // problem. Either way the conclusion is no longer about anything.
        let drawn = plan(vec![abandon()], [grant()].into_iter().collect());
        drawn.authorize(&Digest::of_bytes(b"state-1")).unwrap();

        let error = drawn.authorize(&Digest::of_bytes(b"state-2")).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
        assert!(!drawn.applies_to(&Digest::of_bytes(b"state-2")));
    }

    #[test]
    fn a_plan_that_proposes_nothing_is_refused() {
        // Recording a finding is DocumentOnly, which is a proposal. An empty
        // action list is a plan that was never finished.
        assert!(plan(Vec::new(), BTreeSet::new()).validate().is_err());
        plan(
            vec![RecoveryAction::DocumentOnly {
                detail: "left as-is pending vendor confirmation".into(),
            }],
            BTreeSet::new(),
        )
        .validate()
        .unwrap();
    }

    #[test]
    fn a_plan_with_no_finding_is_refused() {
        let mut unexplained = plan(vec![abandon()], [grant()].into_iter().collect());
        unexplained.finding = "   ".into();
        assert!(unexplained.validate().is_err());
    }

    #[test]
    fn quarantine_is_destructive_and_deliberately_reversible() {
        // Destructive enough to need authority, because a quarantined artifact
        // stops being read — and still examinable afterwards, which deletion
        // would not be.
        let quarantine = RecoveryAction::Quarantine {
            artifact: "pat_a".into(),
            reason: "its journal contradicts the control record".into(),
        };
        assert!(quarantine.is_destructive());
        assert!(plan(vec![quarantine], BTreeSet::new()).validate().is_err());
    }

    #[test]
    fn a_recorded_plan_cannot_be_revised_behind_its_id() {
        // What was proposed at the time is part of the record, including a
        // proposal somebody rejected.
        let directory = tempfile::tempdir().unwrap();
        let store = RecoveryPlanStore::new(directory.path());
        let original = plan(
            vec![RecoveryAction::DocumentOnly {
                detail: "left as-is".into(),
            }],
            BTreeSet::new(),
        );
        store.put(&original).unwrap();
        store.put(&original).unwrap();

        let revised = plan(vec![abandon()], [grant()].into_iter().collect());
        assert!(store.put(&revised).is_err());
        assert_eq!(
            store
                .get(&RecoveryPlanId::parse("rcv_000000000001").unwrap())
                .unwrap(),
            Some(original)
        );
    }

    #[test]
    fn an_incoherent_plan_never_becomes_a_durable_fact() {
        let directory = tempfile::tempdir().unwrap();
        let store = RecoveryPlanStore::new(directory.path());
        assert!(store.put(&plan(vec![abandon()], BTreeSet::new())).is_err());
        assert_eq!(
            store
                .get(&RecoveryPlanId::parse("rcv_000000000001").unwrap())
                .unwrap(),
            None
        );
    }
}
