//! The boundary where an extension's proposal becomes Draft's operation.
//!
//! A tool action returns *proposed* mutations and nothing else. It cannot name
//! an operation id, an actor, a precondition or a plan, because those are the
//! parts that carry authority: whoever writes them decides what Draft is
//! allowed to do and who is recorded as having done it. An extension writes
//! what it wants to happen; Draft decides whether that may happen, under whose
//! authority, against which observed state, and then authors the operation.
//!
//! The schema enforces the split rather than a convention: [`ToolActionResult`]
//! has nowhere to put authority metadata, so a package cannot supply it even by
//! accident, and `deny_unknown_fields` refuses a response that tries.

use crate::dcg::resource::ResourceLocator;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use serde::{Deserialize, Serialize};

/// What a tool action may ask Draft to do to one resource.
///
/// Deliberately a small, closed vocabulary of *outcomes*, not instructions: a
/// proposal says what state a resource should end in, and Draft works out the
/// operation that gets there under its own preconditions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "proposal", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProposedMutation {
    /// The resource should hold this content.
    SetContent {
        locator: ResourceLocator,
        /// UTF-8 content. A tool that needs to propose opaque bytes writes them
        /// into its authorized output scope and proposes them by digest, which
        /// keeps the response bounded and reviewable.
        content: String,
    },
    /// The collection should exist.
    CreateCollection { locator: ResourceLocator },
    /// The resource should be at `to` rather than `from`.
    Relocate {
        from: ResourceLocator,
        to: ResourceLocator,
    },
    /// The resource should not exist.
    Remove {
        locator: ResourceLocator,
        #[serde(default)]
        recursive: bool,
    },
}

impl ProposedMutation {
    /// Every locator this proposal touches, on both sides.
    ///
    /// Both sides matter: a relocation that moved a resource *out of* a
    /// protected scope would otherwise pass a check that only looked at where
    /// it ended up.
    pub fn locators(&self) -> Vec<&ResourceLocator> {
        match self {
            Self::SetContent { locator, .. }
            | Self::CreateCollection { locator }
            | Self::Remove { locator, .. } => vec![locator],
            Self::Relocate { from, to } => vec![from, to],
        }
    }
}

/// Everything a tool action is allowed to return.
///
/// Not a Draft contract: it is an extension's response, validated against the
/// response schema its own package declared. Draft's contract registry governs
/// what Draft persists and transmits, and a tool's findings are neither until
/// Draft has turned them into evidence of its own.
///
/// Note what is absent, and cannot be added by a package: no operation id, no
/// attribution, no preconditions, no mutation plan, no lease, no receipt. A
/// tool reports what it found and what it would like changed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolActionResult {
    /// The tool's own findings, for a person to read. Opaque to Draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// What the tool would like changed. Empty for an inspecting action.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposed_mutations: Vec<ProposedMutation>,
    /// Structured detail the tool wants preserved as evidence. Opaque to Draft,
    /// and never interpreted as an instruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

/// Check a proposal against what the action was authorized to do.
///
/// An `Inspect` action proposing a mutation is refused rather than downgraded
/// to its findings: the package declared one thing and did another, and quietly
/// keeping the half Draft happens to like would hide that.
pub fn check_within_declared_effect(
    action_id: &str,
    effect: &draft_extension_contract::ToolEffect,
    result: &ToolActionResult,
) -> DraftResult<()> {
    use draft_extension_contract::ToolEffect;
    if result.proposed_mutations.is_empty() {
        return Ok(());
    }
    let permitted = match effect {
        ToolEffect::Inspect => false,
        ToolEffect::Transform { output_scope } => output_scope.may_propose_mutations,
    };
    if permitted {
        return Ok(());
    }
    Err(DraftError::new(
        DraftErrorKind::CapabilityNotAuthorized,
        format!(
            "tool action '{action_id}' proposed {} mutation(s), which its declared effect does \
             not permit",
            result.proposed_mutations.len()
        ),
    )
    .with_suggestion(
        "the package must declare `transform` with `may_propose_mutations`, and be reauthorized",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_extension_contract::{OutputScope, ToolEffect};

    fn set(body: &str) -> ProposedMutation {
        ProposedMutation::SetContent {
            locator: ResourceLocator::file(body),
            content: "x".into(),
        }
    }

    #[test]
    fn a_tool_response_has_nowhere_to_put_authority_metadata() {
        // The property this type exists for: a package cannot author an
        // operation id, an actor or a precondition, because the schema refuses
        // the field rather than Draft remembering to ignore it.
        for forged in [
            r#"{"operation_id": "op_forged"}"#,
            r#"{"attribution": {"kind": "task", "id": "tsk_forged"}}"#,
            r#"{"preconditions": []}"#,
            r#"{"plan": {"steps": []}}"#,
            r#"{"proposed_mutations": [], "actor_id": "act_forged"}"#,
        ] {
            let decoded: Result<ToolActionResult, _> = serde_json::from_str(forged);
            assert!(decoded.is_err(), "accepted authority metadata: {forged}");
        }

        // What it may say is accepted.
        let honest: ToolActionResult = serde_json::from_str(
            r#"{"summary":"two files would change",
                "proposed_mutations":[{"proposal":"remove","locator":{"scheme":"file","body":"a"}}]}"#,
        )
        .unwrap();
        assert_eq!(honest.proposed_mutations.len(), 1);
    }

    #[test]
    fn an_inspecting_action_cannot_propose_a_mutation() {
        let proposing = ToolActionResult {
            proposed_mutations: vec![set("a.txt")],
            ..Default::default()
        };
        let error = check_within_declared_effect("ex.pub/look", &ToolEffect::Inspect, &proposing)
            .unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CapabilityNotAuthorized);

        // A transform that did not declare it may not either.
        let withheld = ToolEffect::Transform {
            output_scope: OutputScope {
                max_output_bytes: 1024,
                may_propose_mutations: false,
            },
        };
        assert!(check_within_declared_effect("ex.pub/fix", &withheld, &proposing).is_err());

        // And one that did, may.
        let declared = ToolEffect::Transform {
            output_scope: OutputScope {
                max_output_bytes: 1024,
                may_propose_mutations: true,
            },
        };
        assert!(check_within_declared_effect("ex.pub/fix", &declared, &proposing).is_ok());

        // An inspecting action that proposes nothing is fine, which is the
        // ordinary case and must not be caught by the same check.
        assert!(check_within_declared_effect(
            "ex.pub/look",
            &ToolEffect::Inspect,
            &ToolActionResult::default()
        )
        .is_ok());
    }

    #[test]
    fn a_relocation_exposes_both_sides() {
        let relocate = ProposedMutation::Relocate {
            from: ResourceLocator::file("secrets/key"),
            to: ResourceLocator::file("public/key"),
        };
        // Both, so a move out of a protected scope cannot pass a check that
        // only looked at the destination.
        assert_eq!(
            relocate
                .locators()
                .iter()
                .map(|locator| locator.body.as_str())
                .collect::<Vec<_>>(),
            vec!["secrets/key", "public/key"]
        );
    }
}
