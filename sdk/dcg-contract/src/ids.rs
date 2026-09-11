//! The frozen v1 identifier families.
//!
//! Every Draft-minted identifier carries a short prefix naming what it
//! identifies, so a value is self-describing wherever it appears — in a
//! canonical fact, a CLI argument, a log line or an error message — and a
//! `pat_` can never be silently accepted where a `pub_` was meant.
//!
//! The prefix is part of the identifier, not decoration: parsing requires it,
//! so a bare opaque string is refused rather than adopted into the wrong
//! family. The families are frozen for v1; `pck_`, `vplan_`, `vres_`, `rbp_`,
//! `eap_`, the old `chk_` collision and the old `ws_` project identity are
//! retired and are not parsed here.
//!
//! Identifiers are opaque. Nothing may infer meaning from the bytes after the
//! prefix — not ordering, not creation time, not the resource a `res_` names.

use serde::{Deserialize, Serialize};

use crate::identifier::{validate_segment, IdentifierClass};
use crate::{FormatError, FormatResult};

/// Declares one prefixed identifier family.
macro_rules! dcg_id {
    ($(#[$meta:meta])* $name:ident, $prefix:literal, $what:literal) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// The frozen prefix every value of this family carries.
            pub const PREFIX: &'static str = $prefix;

            /// Parse a value, requiring the family prefix.
            pub fn parse(value: impl Into<String>) -> FormatResult<Self> {
                let value = value.into();
                let Some(body) = value.strip_prefix($prefix) else {
                    return Err(FormatError::Identity(format!(
                        concat!($what, " '{}' must be prefixed '", $prefix, "'"),
                        value
                    )));
                };
                if body.is_empty() {
                    return Err(FormatError::Identity(format!(
                        concat!($what, " '{}' has no body after its prefix"),
                        value
                    )));
                }
                // The body is held to the canonical identifier rules, so it
                // is ASCII, bounded and byte-compared. It is validated without
                // the prefix because `_` is the prefix separator and carries no
                // meaning inside the body — admitting it there would let
                // `pub_pat_x` parse as two different families.
                validate_segment(body, IdentifierClass::Restricted, $what)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl TryFrom<String> for $name {
            type Error = FormatError;

            fn try_from(value: String) -> FormatResult<Self> {
                Self::parse(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> String {
                value.0
            }
        }
    };
}

dcg_id!(
    /// Identifies a Draft project — the unit the Change Graph belongs to.
    ///
    /// This is the identity the retired `ws_` workspace id used to carry.
    ProjectId, "prj_", "project id");
dcg_id!(
    /// Identifies a unit of intended work, independent of any Change.
    TaskId, "tsk_", "task id");
dcg_id!(
    /// Identifies a Resource. Opaque and stable across move and rename: a
    /// Resource that is relocated keeps its identity, because a locator is
    /// where something is, not what it is.
    ResourceId, "res_", "resource id");
dcg_id!(
    /// Identifies one Observation of one Resource's state.
    ObservationId, "obs_", "observation id");
dcg_id!(
    /// Identifies one observation run — the independently addressable
    /// provenance of a batch of Observations and their coverage.
    ObservationRunId, "run_", "observation run id");
dcg_id!(
    /// Identifies a Change: a unit of proposed work with its own lifecycle.
    ChangeId, "chg_", "change id");
dcg_id!(
    /// Identifies one sealed revision of a Change.
    ///
    /// Evidence, Assessments, Reviews, Decisions and Gates bind an *exact*
    /// revision, and that word is mechanical: the id is backed by a
    /// create-once id-to-digest binding verified on every load, so it can
    /// never degrade into equality of an opaque string.
    ChangeRevisionId, "rev_", "change revision id");
dcg_id!(
    /// Identifies a planned or executed Operation.
    OperationId, "op_", "operation id");
dcg_id!(
    /// Identifies a Change workspace — the mutable surface work happens on.
    ///
    /// Distinct from [`ProjectId`]: the retired `ws_` family meant *project*,
    /// which is the single most likely place to misread this rename.
    WorkspaceId, "wsp_", "workspace id");
dcg_id!(
    /// Identifies a checkpoint of a workspace.
    CheckpointId, "ckp_", "checkpoint id");
dcg_id!(
    /// Identifies a piece of Evidence bound to an exact ChangeRevision.
    EvidenceId, "evd_", "evidence id");
dcg_id!(
    /// Identifies an Assessment bound to an exact ChangeRevision.
    AssessmentId, "asm_", "assessment id");
dcg_id!(
    /// Identifies a Review.
    ReviewId, "rvw_", "review id");
dcg_id!(
    /// Identifies an immutable recorded Decision.
    DecisionId, "dec_", "decision id");
dcg_id!(
    /// The navigation handle for an accepted Baseline.
    ///
    /// Not the Baseline's identity: that is [`crate::BaselineId`], the digest
    /// of its manifest. This is the short handle a person types.
    BaselineIdentifier, "bas_", "baseline handle");
dcg_id!(
    /// Identifies an immutable AuthorityGrant.
    AuthorityGrantId, "auth_", "authority grant id");
dcg_id!(
    /// Identifies a durable Receipt. Preallocated before the crash-sensitive
    /// boundary it attests, so recovery can finalize exactly one.
    ReceiptId, "rcp_", "receipt id");
dcg_id!(
    /// Identifies one appended Activity Ledger event.
    ActivityEventId, "evt_", "activity event id");
dcg_id!(
    /// Identifies an actor — human, agent or service.
    ActorId, "act_", "actor id");
dcg_id!(
    /// Identifies one execution.
    ExecutionId, "exe_", "execution id");
dcg_id!(
    /// Identifies an immutable RecoveryPlan.
    RecoveryPlanId, "rcv_", "recovery plan id");
dcg_id!(
    /// Identifies a ProviderBinding: a project's configured attachment to one
    /// provider. Revisioned at runtime; only the identity is portable.
    ProviderBindingId, "pbd_", "provider binding id");
dcg_id!(
    /// Identifies one Promotion.
    PromotionId, "pro_", "promotion id");
dcg_id!(
    /// Identifies one Publication: an independently identified intent to
    /// deliver an exact Baseline to one exact provider route.
    PublicationId, "pub_", "publication id");
dcg_id!(
    /// Identifies one attempt at a Publication.
    ///
    /// Its existence is never proof that an external request was made: a
    /// staged attempt artifact and a durably dispatched attempt are different
    /// facts.
    PublicationAttemptId, "pat_", "publication attempt id");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prefix_is_required_so_a_bare_string_is_never_adopted() {
        assert!(PublicationId::parse("pub_a1b2c3").is_ok());
        assert!(PublicationId::parse("a1b2c3").is_err());
        assert!(PublicationId::parse("pub_").is_err());
    }

    #[test]
    fn families_do_not_accept_one_another() {
        // The single most valuable property here: an attempt id cannot be
        // read as a publication id, however similar the two look.
        assert!(PublicationId::parse("pat_a1b2c3").is_err());
        assert!(PublicationAttemptId::parse("pub_a1b2c3").is_err());
        assert!(ProjectId::parse("wsp_a1b2c3").is_err());
        assert!(WorkspaceId::parse("prj_a1b2c3").is_err());
    }

    #[test]
    fn the_retired_families_are_not_parsed_by_anything() {
        // `pck_`, `chk_`, `ws_` and friends belong to the retired ontology.
        // Naming them here is how this test proves nothing parses them.
        // retired-architecture-ok: naming them is how the test proves it.
        for retired in ["pck_a1", "chk_a1", "ws_a1", "vplan_a1", "rbp_a1", "eap_a1"] {
            assert!(ChangeId::parse(retired).is_err());
            assert!(CheckpointId::parse(retired).is_err());
            assert!(ProjectId::parse(retired).is_err());
        }
    }

    #[test]
    fn identifiers_are_byte_compared_and_bounded() {
        assert!(ResourceId::parse("res_ABC").is_err(), "no case folding");
        assert!(ResourceId::parse("res_a b").is_err(), "no whitespace");
        assert!(ResourceId::parse(format!("res_{}", "a".repeat(200))).is_err());
    }

    #[test]
    fn the_wire_form_round_trips() {
        let id = ObservationRunId::parse("run_9f8e7d").unwrap();
        let encoded = serde_json::to_string(&id).unwrap();
        assert_eq!(encoded, "\"run_9f8e7d\"");
        assert_eq!(
            serde_json::from_str::<ObservationRunId>(&encoded).unwrap(),
            id
        );
        assert!(serde_json::from_str::<ObservationRunId>("\"obs_9f8e7d\"").is_err());
    }

    #[test]
    fn ordering_is_deterministic_for_canonical_sorting() {
        let mut ids = [
            ResourceId::parse("res_c").unwrap(),
            ResourceId::parse("res_a").unwrap(),
            ResourceId::parse("res_b").unwrap(),
        ];
        ids.sort();
        assert_eq!(
            ids.iter().map(ResourceId::as_str).collect::<Vec<_>>(),
            ["res_a", "res_b", "res_c"]
        );
    }
}
