use draft_core::extension::{
    ActiveContributions, Contributed, ContributionPayload, ExtensionCapabilityKind,
    ExtensionContributionKind, ExtensionContributionSource, ExtensionPermission,
    InstalledExtension, ProducerRef, WithheldCapability,
};
use draft_core::project::home::DraftGlobalStore;
use draft_core::support::error::DraftResult;

/// Reads contributions from the extensions installed in the global store.
#[derive(Debug, Clone, Copy, Default)]
pub struct InstalledExtensions;

impl ExtensionContributionSource for InstalledExtensions {
    fn active_contributions(&self) -> ActiveContributions {
        // A store that cannot be read contributes nothing. Draft stays usable
        // as a generic platform rather than failing an unrelated operation
        // because the extension store is unavailable.
        resolve().unwrap_or_default()
    }
}

/// Resolve every active contribution, applying authorization as it goes.
pub fn resolve() -> DraftResult<ActiveContributions> {
    let home = DraftGlobalStore::locate()?;
    let mut active = ActiveContributions::default();

    for installed in crate::extension::list()? {
        if !installed.enabled || crate::catalog::package_is_revoked(&installed)? {
            continue;
        }
        let may_execute =
            crate::authorization::authorizes(&installed, ExtensionPermission::ProcessExecute)
                .unwrap_or(false);

        // The artifact each contribution was accepted under, recorded once so
        // every derived result can name it later — after this extension has
        // been updated or removed.
        active
            .attestations
            .insert(installed.id().to_string(), producer_of(&installed));

        for contribution in &installed.manifest.contributions {
            let path = home
                .extensions_dir()
                .join("packages")
                .join(installed.id())
                .join(&contribution.path);
            let Ok(bytes) = std::fs::read(&path) else {
                // A package whose declared file is unreadable contributes
                // nothing from it; installation already validated that the
                // file was there, so this is a store problem, not a reason to
                // fail the caller's operation.
                continue;
            };
            let Ok(payload) = ContributionPayload::decode(contribution.kind, &bytes) else {
                continue;
            };
            if payload.validate().is_err() {
                continue;
            }
            // Every identifier a contribution mints must live in a namespace
            // the declaring package owns. Without this a package could claim
            // another publisher's class, check or intent id and have its rules
            // silently apply to their work.
            if payload
                .contributed_ids()
                .iter()
                .any(|id| !id.is_owned_by(installed.id()))
            {
                continue;
            }
            absorb(
                &mut active,
                &installed,
                contribution.kind,
                payload,
                may_execute,
            );
        }
    }

    Ok(active)
}

/// The artifact identity one installed extension contributes under.
fn producer_of(installed: &InstalledExtension) -> ProducerRef {
    let attestation = installed.provenance.attestation(&installed.content_hash);
    ProducerRef {
        extension_id: installed.id().to_string(),
        extension_version: installed.manifest.version.clone(),
        package_digest: attestation.package_digest.clone(),
        attestation_digest: attestation.attestation_digest,
    }
}

/// Fold one decoded contribution into the active set.
///
/// Command-bearing contributions are the one thing gated here. A package whose
/// artifact is not authorized for `process.execute` has its declared operations
/// withheld *before* Core sees them, so Core structurally cannot run something
/// that was never authorized. What was withheld is recorded, because a quiet
/// capability and an absent one need to look different to whoever is reading.
fn absorb(
    active: &mut ActiveContributions,
    installed: &InstalledExtension,
    kind: ExtensionContributionKind,
    payload: ContributionPayload,
    may_execute: bool,
) {
    // Every contribution carries the id of the extension that supplied it, so
    // Core can say who agreed on a classification and name the candidates when
    // two publishers disagree.
    let from = installed.id();
    let withhold = |active: &mut ActiveContributions, capability| {
        active.withheld.push(WithheldCapability {
            extension_id: from.to_string(),
            capability,
            missing_permissions: vec![ExtensionPermission::ProcessExecute],
        });
    };

    match payload {
        ContributionPayload::ResourceAdapter(adapter) => {
            // An adapter that cannot run its declared mechanism cannot observe
            // anything, so it is withheld whole rather than half-installed.
            if adapter.requires_execution() && !may_execute {
                withhold(active, ExtensionCapabilityKind::ResourceAdapter);
                return;
            }
            active
                .resource_adapters
                .push(Contributed::new(from, *adapter));
        }
        ContributionPayload::ResourceClassification(rule) => {
            active.classifications.push(Contributed::new(from, rule))
        }
        ContributionPayload::Comparison(comparison) => {
            if comparison.requires_execution() && !may_execute {
                withhold(active, ExtensionCapabilityKind::Comparison);
                return;
            }
            active.comparisons.push(Contributed::new(from, *comparison));
        }
        ContributionPayload::ElementExtraction(extraction) => {
            if extraction.requires_execution() && !may_execute {
                withhold(active, ExtensionCapabilityKind::ElementExtraction);
                return;
            }
            active
                .element_extractors
                .push(Contributed::new(from, *extraction));
        }
        ContributionPayload::Presentation(presentation) => {
            // Declarative by construction: a presentation binding names a
            // platform engine and can never carry a command.
            active
                .presentations
                .push(Contributed::new(from, presentation))
        }
        ContributionPayload::ToolAction(action) => {
            if !may_execute {
                withhold(active, ExtensionCapabilityKind::ToolAction);
                return;
            }
            active.tool_actions.push(Contributed::new(from, *action));
        }
        ContributionPayload::Verification(verification) => {
            // Checks are withheld individually: an unauthorized command-backed
            // check should not silence an authorized engine-backed one from the
            // same package.
            let (runnable, withheld_any) = split_checks(*verification, may_execute);
            if withheld_any {
                withhold(active, ExtensionCapabilityKind::Verification);
            }
            if !runnable.checks.is_empty() {
                active.verifications.push(Contributed::new(from, runnable));
            }
        }
        ContributionPayload::RiskRules(rules) => {
            active.risk_rules.push(Contributed::new(from, rules))
        }
        ContributionPayload::PolicyPreset(preset) => {
            active.policies.push(Contributed::new(from, preset))
        }
        ContributionPayload::IntentVocabulary(vocabulary) => active
            .intent_vocabularies
            .push(Contributed::new(from, vocabulary)),
        ContributionPayload::TaskTemplate(template) => active
            .task_templates
            .push(Contributed::new(from, *template)),
        ContributionPayload::CandidatePreset(preset) => {
            // Pure data by construction. A preset names a tool action; it can
            // never itself cause execution, which is why it needs no gate here.
            active
                .candidate_presets
                .push(Contributed::new(from, *preset))
        }
        ContributionPayload::Documentation(_) => {
            debug_assert_eq!(
                kind,
                ExtensionContributionKind::Documentation,
                "documentation payload decoded from another kind"
            );
        }
    }
}

/// Split a verification contribution into what may run and what may not.
///
/// Returns the runnable half and whether anything was withheld. A withheld
/// check is dropped rather than kept without its command: keeping it would let
/// it aggregate as `NotEvaluated` from a package that is perfectly capable of
/// running it, which misdescribes the situation. The `WithheldCapability`
/// record is what says the fix is to authorize, not to install.
fn split_checks(
    contribution: draft_core::extension::VerificationContribution,
    may_execute: bool,
) -> (draft_core::extension::VerificationContribution, bool) {
    if may_execute {
        return (contribution, false);
    }
    let total = contribution.checks.len();
    let runnable: Vec<draft_core::extension::VerificationCheck> = contribution
        .checks
        .into_iter()
        .filter(|check| check.operation.executor.command().is_none())
        .collect();
    let withheld_any = runnable.len() != total;
    (
        draft_core::extension::VerificationContribution { checks: runnable },
        withheld_any,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_core::extension::{
        CheckRequirement, CheckSelection, EngineId, Executor, MechanismOperation, NamespacedId,
        RawResourcePredicate, ResourcePredicate, SchemaRef, StructuredCommand, VerificationCheck,
        VerificationContribution,
    };

    fn id(value: &str) -> NamespacedId {
        NamespacedId::parse(value).unwrap()
    }

    fn schema(value: &str) -> SchemaRef {
        SchemaRef {
            schema_id: id(value),
            revision: 1,
        }
    }

    fn check(name: &str, executor: Executor) -> VerificationCheck {
        VerificationCheck {
            check_id: id(name),
            display_name: name.into(),
            applies_to: ResourcePredicate::Raw {
                of: RawResourcePredicate::LocatorScheme {
                    equals: "file".into(),
                },
            },
            requirement: CheckRequirement::Required,
            selection: CheckSelection::Whole,
            operation: MechanismOperation {
                request_contract: schema("example.pub/request"),
                response_contract: schema("example.pub/response"),
                max_response_bytes: 4096,
                executor,
            },
        }
    }

    fn commanded(name: &str) -> VerificationCheck {
        check(
            name,
            Executor::Command {
                command: StructuredCommand {
                    program: "example-check".into(),
                    args: vec![],
                    cwd: None,
                    timeout_ms: None,
                },
            },
        )
    }

    fn engined(name: &str) -> VerificationCheck {
        check(
            name,
            Executor::Engine {
                engine: EngineId::WholeResource,
                engine_revision: 1,
                config: serde_json::json!({}),
            },
        )
    }

    #[test]
    fn an_authorized_package_keeps_every_check() {
        let contribution = VerificationContribution {
            checks: vec![commanded("example.pub/a"), engined("example.pub/b")],
        };
        let (runnable, withheld) = split_checks(contribution, true);
        assert_eq!(runnable.checks.len(), 2);
        assert!(!withheld);
    }

    #[test]
    fn an_unauthorized_command_check_is_withheld_without_silencing_its_neighbours() {
        let contribution = VerificationContribution {
            checks: vec![commanded("example.pub/a"), engined("example.pub/b")],
        };
        let (runnable, withheld) = split_checks(contribution, false);
        // The engine-backed check needs no grant and survives; only the one
        // that would have launched a process is withheld.
        assert_eq!(runnable.checks.len(), 1);
        assert_eq!(runnable.checks[0].check_id, id("example.pub/b"));
        assert!(withheld, "the withheld capability must be reported");
    }

    #[test]
    fn a_package_whose_checks_all_need_execution_contributes_none() {
        let contribution = VerificationContribution {
            checks: vec![commanded("example.pub/a")],
        };
        let (runnable, withheld) = split_checks(contribution, false);
        assert!(runnable.checks.is_empty());
        assert!(withheld);
    }
}
