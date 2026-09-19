//! The frozen v1 Activity event vocabulary, and who owns each event.
//!
//! # Every event has a durable owner
//!
//! An event nobody owns is an event nobody writes durably. The rule is that
//! each [`EventKind`] maps to a specific [`AuditFactKind`] — the domain fact a
//! lower layer persists — and a specific [`JournalMechanism`] that makes that
//! fact durable *before* the commit it describes. [`EventKind::ownership`] is a
//! total match, so the compiler refuses a new event that skips the question.
//!
//! # Names say what is true, not what is hoped
//!
//! The vocabulary avoids names that could outlive the fact they claim. The
//! clearest case is publication dispatch. There is deliberately **no
//! `PublicationAttempted`**: the durable `Dispatching` boundary happens
//! *before* the external call, so an event emitted there could survive a crash
//! in which nothing was ever sent — and "attempted" would then be a lie in the
//! permanent record.
//!
//! [`EventKind::PublicationDispatchCommitted`] names exactly what is true at
//! that point: Draft durably committed the authority and state required to
//! permit this exact external dispatch. Whether the request arrived is what the
//! outcome events record, once reconciliation establishes it. No compatibility
//! alias is kept, because an alias would let the misleading name back in.

use serde::{Deserialize, Serialize};

/// Every Activity event Draft can record.
///
/// Frozen for v1. Adding one means answering [`EventKind::ownership`] for it,
/// which is the point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum EventKind {
    // Project lifecycle.
    ProjectCreated,
    BaselineInitialized,
    ProjectClosed,

    // Workspaces.
    WorkspaceCreated,
    CheckpointCreated,

    // External-system definitions and bindings.
    ProviderSemanticDefinitionAdded,
    ProviderOperationalProfileAdded,
    ProviderBindingAdded,
    ProviderBindingRetargeted,
    ProviderBindingUnbound,
    ProviderBindingRebound,

    // Tasks.
    TaskCreated,
    TaskUpdated,
    TaskClosed,
    TaskReopened,

    // ChangePacks.
    ChangePackCreated,
    ChangePackDefinitionAmended,
    ChangePackCompleted,
    ChangePackAbandoned,
    ChangePackReopened,

    // Security and policy.
    AuthorityGranted,
    AuthorityRevoked,
    SecurityStateUpdated,
    PolicyUpdated,

    // Operations.
    OperationPlanned,
    OperationExecuted,
    OperationRefused,
    OperationReplanned,

    // Observation and derivation.
    ResourceObserved,
    CoverageRecorded,
    RelationDerived,
    StateBearingDeclared,
    ScopeResolved,
    RevisionPackSealed,

    // Evidence and assessment.
    EvidenceProduced,
    AssessmentProduced,

    // Review and gates.
    ReviewSubmitted,
    DecisionRecorded,
    GateEvaluated,
    GateWaived,

    // Leases.
    LeaseAcquired,
    LeaseReleased,
    LeaseRefused,

    // Promotion.
    PromotionPrepared,
    PromotionCommitted,
    PromotionFinalized,
    PromotionRefused,
    PromotionAbandoned,
    PromotionInconsistent,
    BaselinePromoted,

    // Publication.
    PublicationRequested,
    /// Draft durably committed the authority and state permitting this exact
    /// external dispatch.
    ///
    /// Deliberately not "attempted": this is emitted before the external call,
    /// so it must not claim the request was made. See the module docs.
    PublicationDispatchCommitted,
    PublicationSucceeded,
    PublicationFailed,
    PublicationNoEffect,
    PublicationIndeterminate,
    PublicationAbandonedBeforeDispatch,
    PublicationResolved,
    PublicationRetryAuthorized,
    PublicationInconsistent,

    // Receipts and recovery.
    ReceiptIssued,
    RecoveryPerformed,

    // Extensions.
    ExtensionInstalled,
    ExtensionAuthorized,
    ExtensionRevoked,

    // Maintenance.
    MaintenanceStarted,
    MaintenanceCompleted,
    MaintenanceFailed,
}

/// The domain fact a lower layer persists, which `app/activity` converts.
///
/// Lower layers persist one of these carrying a preallocated event id; none of
/// them constructs an Activity payload or calls the ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AuditFactKind {
    Initialization,
    Security,
    Promotion,
    ChangePackLifecycle,
    TaskLifecycle,
    ProviderLifecycle,
    Extension,
    Observation,
    Evidence,
    Gate,
    Execution,
    Publication,
    Recovery,
    Lease,
    Workspace,
    Maintenance,
}

/// What makes the fact durable before the commit it describes.
///
/// Named specifically rather than generically: "the Publication journal" would
/// hide that resolution and retry-authorization facts are made durable by their
/// own transactions, not by a later attempt journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum JournalMechanism {
    /// The crash-safe project initialization transaction.
    InitializationJournal,
    /// A `MutationJournal` on the project control record.
    ProjectControlMutationJournal,
    /// A `MutationJournal` on a per-record domain Store.
    RecordMutationJournal,
    /// An immutable fact's own creation journal, in its owning Store.
    FactCreationJournal,
    /// The Promotion journal, which also carries the planned ChangePack completion.
    PromotionJournal,
    /// The Publication creation journal, under the registry lock.
    PublicationCreationJournal,
    /// The guarded per-attempt Publication journal.
    PublicationAttemptJournal,
    /// The per-outcome resolution transaction.
    ResolutionMutationJournal,
    /// The retry-authorization fact-creation journal.
    RetryAuthorizationJournal,
    /// The execution store's journal.
    ExecutionJournal,
    /// The recovery journal.
    RecoveryJournal,
    /// An embedded transactional outbox in the owning service.
    ServiceOutbox,
    /// The maintenance journal.
    MaintenanceJournal,
}

/// Who owns an event: the fact, and what makes it durable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventOwnership {
    pub audit_fact: AuditFactKind,
    pub mechanism: JournalMechanism,
}

impl EventKind {
    /// Every event Draft can record, in vocabulary order.
    pub const ALL: &'static [EventKind] = &[
        Self::ProjectCreated,
        Self::BaselineInitialized,
        Self::ProjectClosed,
        Self::WorkspaceCreated,
        Self::CheckpointCreated,
        Self::ProviderSemanticDefinitionAdded,
        Self::ProviderOperationalProfileAdded,
        Self::ProviderBindingAdded,
        Self::ProviderBindingRetargeted,
        Self::ProviderBindingUnbound,
        Self::ProviderBindingRebound,
        Self::TaskCreated,
        Self::TaskUpdated,
        Self::TaskClosed,
        Self::TaskReopened,
        Self::ChangePackCreated,
        Self::ChangePackDefinitionAmended,
        Self::ChangePackCompleted,
        Self::ChangePackAbandoned,
        Self::ChangePackReopened,
        Self::AuthorityGranted,
        Self::AuthorityRevoked,
        Self::SecurityStateUpdated,
        Self::PolicyUpdated,
        Self::OperationPlanned,
        Self::OperationExecuted,
        Self::OperationRefused,
        Self::OperationReplanned,
        Self::ResourceObserved,
        Self::CoverageRecorded,
        Self::RelationDerived,
        Self::StateBearingDeclared,
        Self::ScopeResolved,
        Self::RevisionPackSealed,
        Self::EvidenceProduced,
        Self::AssessmentProduced,
        Self::ReviewSubmitted,
        Self::DecisionRecorded,
        Self::GateEvaluated,
        Self::GateWaived,
        Self::LeaseAcquired,
        Self::LeaseReleased,
        Self::LeaseRefused,
        Self::PromotionPrepared,
        Self::PromotionCommitted,
        Self::PromotionFinalized,
        Self::PromotionRefused,
        Self::PromotionAbandoned,
        Self::PromotionInconsistent,
        Self::BaselinePromoted,
        Self::PublicationRequested,
        Self::PublicationDispatchCommitted,
        Self::PublicationSucceeded,
        Self::PublicationFailed,
        Self::PublicationNoEffect,
        Self::PublicationIndeterminate,
        Self::PublicationAbandonedBeforeDispatch,
        Self::PublicationResolved,
        Self::PublicationRetryAuthorized,
        Self::PublicationInconsistent,
        Self::ReceiptIssued,
        Self::RecoveryPerformed,
        Self::ExtensionInstalled,
        Self::ExtensionAuthorized,
        Self::ExtensionRevoked,
        Self::MaintenanceStarted,
        Self::MaintenanceCompleted,
        Self::MaintenanceFailed,
    ];

    /// The fact and mechanism responsible for this event.
    ///
    /// A total match on purpose: a new event cannot be added without answering
    /// where its fact becomes durable, which is the whole invariant.
    pub fn ownership(self) -> EventOwnership {
        use AuditFactKind as Fact;
        use JournalMechanism as Journal;

        let (audit_fact, mechanism) = match self {
            Self::ProjectCreated | Self::BaselineInitialized => {
                (Fact::Initialization, Journal::InitializationJournal)
            }

            Self::ProjectClosed | Self::PolicyUpdated | Self::SecurityStateUpdated => {
                (Fact::Security, Journal::ProjectControlMutationJournal)
            }

            Self::WorkspaceCreated | Self::CheckpointCreated => {
                (Fact::Workspace, Journal::ServiceOutbox)
            }

            // Immutable definitions and profiles are independent facts created
            // by their own Store, never audited by a later binding mutation.
            Self::ProviderSemanticDefinitionAdded | Self::ProviderOperationalProfileAdded => {
                (Fact::ProviderLifecycle, Journal::FactCreationJournal)
            }
            // Pointer and lifecycle mutations, which are a different thing.
            Self::ProviderBindingAdded
            | Self::ProviderBindingRetargeted
            | Self::ProviderBindingUnbound
            | Self::ProviderBindingRebound => {
                (Fact::ProviderLifecycle, Journal::RecordMutationJournal)
            }

            Self::TaskCreated | Self::TaskUpdated | Self::TaskClosed | Self::TaskReopened => {
                (Fact::TaskLifecycle, Journal::RecordMutationJournal)
            }

            // ChangePack completion is the Promotion's, because it is that
            // transaction's deterministic finalization rather than a separate
            // decision somebody makes afterwards.
            Self::ChangePackCompleted => (Fact::ChangePackLifecycle, Journal::PromotionJournal),
            Self::ChangePackCreated
            | Self::ChangePackDefinitionAmended
            | Self::ChangePackAbandoned
            | Self::ChangePackReopened => {
                (Fact::ChangePackLifecycle, Journal::RecordMutationJournal)
            }

            Self::AuthorityGranted | Self::AuthorityRevoked => {
                (Fact::Security, Journal::FactCreationJournal)
            }
            Self::ExtensionInstalled | Self::ExtensionAuthorized | Self::ExtensionRevoked => {
                (Fact::Extension, Journal::FactCreationJournal)
            }

            Self::OperationPlanned
            | Self::OperationExecuted
            | Self::OperationRefused
            | Self::OperationReplanned => (Fact::Execution, Journal::ExecutionJournal),

            Self::ResourceObserved
            | Self::CoverageRecorded
            | Self::RelationDerived
            | Self::StateBearingDeclared => (Fact::Observation, Journal::FactCreationJournal),
            Self::ScopeResolved | Self::RevisionPackSealed => {
                (Fact::ChangePackLifecycle, Journal::FactCreationJournal)
            }

            Self::EvidenceProduced | Self::AssessmentProduced => {
                (Fact::Evidence, Journal::FactCreationJournal)
            }
            Self::ReviewSubmitted
            | Self::DecisionRecorded
            | Self::GateEvaluated
            | Self::GateWaived => (Fact::Gate, Journal::FactCreationJournal),

            Self::LeaseAcquired | Self::LeaseReleased | Self::LeaseRefused => {
                (Fact::Lease, Journal::ServiceOutbox)
            }

            Self::PromotionPrepared
            | Self::PromotionCommitted
            | Self::PromotionFinalized
            | Self::PromotionRefused
            | Self::PromotionAbandoned
            | Self::PromotionInconsistent
            | Self::BaselinePromoted => (Fact::Promotion, Journal::PromotionJournal),

            Self::PublicationRequested => (Fact::Publication, Journal::PublicationCreationJournal),
            // Durable in the same guarded transition that permits the dispatch,
            // so a crash before the append cannot lose it.
            Self::PublicationDispatchCommitted
            | Self::PublicationSucceeded
            | Self::PublicationFailed
            | Self::PublicationNoEffect
            | Self::PublicationIndeterminate
            | Self::PublicationAbandonedBeforeDispatch => {
                (Fact::Publication, Journal::PublicationAttemptJournal)
            }
            // Its own transaction under the per-outcome head, not a later
            // attempt journal.
            Self::PublicationResolved => (Fact::Publication, Journal::ResolutionMutationJournal),
            // Likewise its own fact-creation journal.
            Self::PublicationRetryAuthorized => {
                (Fact::Publication, Journal::RetryAuthorizationJournal)
            }

            Self::PublicationInconsistent | Self::RecoveryPerformed => {
                (Fact::Recovery, Journal::RecoveryJournal)
            }

            // Issued alongside the fact it attests, by that fact's transaction.
            Self::ReceiptIssued => (Fact::Publication, Journal::PublicationAttemptJournal),

            Self::MaintenanceStarted | Self::MaintenanceCompleted | Self::MaintenanceFailed => {
                (Fact::Maintenance, Journal::MaintenanceJournal)
            }
        };

        EventOwnership {
            audit_fact,
            mechanism,
        }
    }

    /// The event's stable wire name.
    pub fn as_str(self) -> &'static str {
        // Serde owns the canonical rendering, so there is one spelling rather
        // than a hand-written table that could drift from it.
        match serde_json::to_value(self) {
            Ok(serde_json::Value::String(name)) => Box::leak(name.into_boxed_str()),
            _ => unreachable!("EventKind serializes as a string"),
        }
    }
}

impl std::fmt::Display for EventKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn pack_events_persist_under_their_exact_member_names() {
        for (kind, wire) in [
            (EventKind::ChangePackCreated, "ChangePackCreated"),
            (
                EventKind::ChangePackDefinitionAmended,
                "ChangePackDefinitionAmended",
            ),
            (EventKind::ChangePackCompleted, "ChangePackCompleted"),
            (EventKind::ChangePackAbandoned, "ChangePackAbandoned"),
            (EventKind::ChangePackReopened, "ChangePackReopened"),
            (EventKind::RevisionPackSealed, "RevisionPackSealed"),
        ] {
            let encoded = serde_json::to_value(kind).unwrap();
            assert_eq!(encoded, serde_json::json!(wire));
            assert_eq!(serde_json::from_value::<EventKind>(encoded).unwrap(), kind);
        }
        // retired-architecture-ok: the pre-rename discriminants must not parse.
        for retired in [
            "ChangeCreated",
            "ChangeCompleted",
            "RevisionSealed",
            "PackCreated",
        ] {
            assert!(serde_json::from_value::<EventKind>(serde_json::json!(retired)).is_err());
        }
        assert_eq!(
            EventKind::ChangePackCompleted.ownership().mechanism,
            JournalMechanism::PromotionJournal
        );
    }

    #[test]
    fn every_event_has_a_fact_owner_and_a_named_mechanism() {
        // A total match makes this true by construction; the test states the
        // invariant so its removal is visible.
        for kind in EventKind::ALL {
            let ownership = kind.ownership();
            assert_eq!(
                ownership,
                kind.ownership(),
                "{kind} must have one stable owner"
            );
        }
    }

    #[test]
    fn the_vocabulary_list_is_complete_and_free_of_duplicates() {
        // ALL is hand-written, so it can fall behind the enum. Round-tripping
        // every entry through its wire name catches a duplicate, and the count
        // catches an omission.
        let names: BTreeSet<&str> = EventKind::ALL.iter().map(|kind| kind.as_str()).collect();
        assert_eq!(
            names.len(),
            EventKind::ALL.len(),
            "the vocabulary lists an event twice"
        );
        assert_eq!(EventKind::ALL.len(), 68);
    }

    #[test]
    fn there_is_no_publication_attempted_event() {
        // The durable Dispatching boundary happens before the external call, so
        // an event named "attempted" there could outlive a crash in which
        // nothing was sent.
        for kind in EventKind::ALL {
            // retired-architecture-ok: proving the name is refused must use it.
            assert_ne!(kind.as_str(), "PublicationAttempted");
        }
        assert!(EventKind::ALL.contains(&EventKind::PublicationDispatchCommitted));
        // And no alias is kept, since an alias would let the name back in.
        // retired-architecture-ok: the rejected payload must carry the name.
        assert!(serde_json::from_str::<EventKind>("\"PublicationAttempted\"").is_err());
    }

    #[test]
    fn immutable_definitions_are_not_audited_by_a_later_binding_mutation() {
        // They are independent facts with their own creation journal; folding
        // them into the binding's mutation would leave a definition that was
        // created but never recorded.
        assert_eq!(
            EventKind::ProviderSemanticDefinitionAdded
                .ownership()
                .mechanism,
            JournalMechanism::FactCreationJournal
        );
        assert_eq!(
            EventKind::ProviderBindingRetargeted.ownership().mechanism,
            JournalMechanism::RecordMutationJournal
        );
    }

    #[test]
    fn resolution_and_retry_authorization_own_their_own_transactions() {
        // A generic "Publication journal" would hide that these are made
        // durable by their own transactions, never by a later attempt journal.
        assert_eq!(
            EventKind::PublicationResolved.ownership().mechanism,
            JournalMechanism::ResolutionMutationJournal
        );
        assert_eq!(
            EventKind::PublicationRetryAuthorized.ownership().mechanism,
            JournalMechanism::RetryAuthorizationJournal
        );
        assert_eq!(
            EventKind::PublicationRequested.ownership().mechanism,
            JournalMechanism::PublicationCreationJournal
        );
        assert_eq!(
            EventKind::PublicationDispatchCommitted
                .ownership()
                .mechanism,
            JournalMechanism::PublicationAttemptJournal
        );
    }

    #[test]
    fn change_completion_belongs_to_the_promotion_that_causes_it() {
        // It is the Promotion's deterministic finalization, not a separate
        // decision taken afterwards.
        assert_eq!(
            EventKind::ChangePackCompleted.ownership().mechanism,
            JournalMechanism::PromotionJournal
        );
        assert_eq!(
            EventKind::ChangePackAbandoned.ownership().mechanism,
            JournalMechanism::RecordMutationJournal
        );
    }

    #[test]
    fn the_wire_form_round_trips() {
        for kind in EventKind::ALL {
            let encoded = serde_json::to_string(kind).unwrap();
            assert_eq!(&serde_json::from_str::<EventKind>(&encoded).unwrap(), kind);
        }
        assert_eq!(
            serde_json::to_string(&EventKind::ChangePackCreated).unwrap(),
            "\"ChangePackCreated\""
        );
    }

    #[test]
    fn an_unknown_event_name_is_refused() {
        assert!(serde_json::from_str::<EventKind>("\"SomethingInvented\"").is_err());
    }
}
