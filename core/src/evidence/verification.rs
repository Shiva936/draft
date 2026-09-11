//! Compositional verification, and the five outcomes it can have.
//!
//! Verification is a keyed union of independently named checks: two extensions
//! contributing different check ids both run, and their evidence aggregates.
//! Only a repeated id is ambiguous, and only for that id.
//!
//! The five states exist because collapsing them loses the distinction a gate
//! depends on. In particular `checks.is_empty()` can never mean
//! [`VerificationState::Passed`]: "nothing was checked" is not "everything
//! passed", and a submission gate that cannot tell them apart approves changes
//! nobody verified.

use crate::dcg::change_store::RevisionRecord;
use crate::extension::provenance::ProducerRef;
use crate::provenance::derived::DerivationInputs;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing;
use draft_extension_contract::{
    CheckRequirement, CheckSelection, NamespacedId, StructuredCommand, VerificationStateName,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The Core semantics that aggregate individual check outcomes into one state.
pub const VERIFICATION_AGGREGATOR_REVISION: u32 = 1;

/// What verification established.
///
/// Five distinct facts, none of which is a default for another:
///
/// * `Passed` — at least one required applicable check ran, and every required
///   applicable check passed.
/// * `Failed` — a required applicable check ran and failed.
/// * `Unavailable` — a check is required and applicable, but no usable
///   authorized capability exists to run it.
/// * `NotEvaluated` — a capability existed or was selected, but evaluation did
///   not complete: a timeout, an ambiguity, a refusal, an invalid response.
/// * `NotApplicable` — applicability was determined, and no check legitimately
///   applies to this subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VerificationState {
    Passed,
    Failed {
        failures: Vec<String>,
    },
    Unavailable {
        gaps: Vec<crate::extension::CapabilityGap>,
    },
    NotEvaluated {
        gaps: Vec<crate::extension::CapabilityGap>,
    },
    NotApplicable {
        reason: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        evidence: Vec<String>,
    },
}

impl VerificationState {
    /// Whether this state satisfies a gate on its own.
    ///
    /// Only `Passed` does. Every other state — including `NotApplicable` — needs
    /// an explicit policy decision, because whether "nothing applies here" is
    /// acceptable is a policy question, not a Core one.
    pub fn satisfies_gate_unconditionally(&self) -> bool {
        matches!(self, Self::Passed)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed { .. } => "failed",
            Self::Unavailable { .. } => "unavailable",
            Self::NotEvaluated { .. } => "not_evaluated",
            Self::NotApplicable { .. } => "not_applicable",
        }
    }
}

/// How one check ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CheckOutcome {
    Passed,
    Failed {
        detail: String,
    },
    /// No usable authorized capability to run it.
    Unavailable {
        detail: String,
    },
    /// Selected, but evaluation did not complete.
    NotEvaluated {
        detail: String,
    },
}

/// One check that was selected, with everything needed to audit its result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationCheckResult {
    pub check_id: NamespacedId,
    pub display_name: String,
    pub requirement: CheckRequirement,
    pub selection: CheckSelection,
    /// Why this check was selected, in the caller's own words.
    pub reason: String,
    pub outcome: CheckOutcome,
    /// The artifact that contributed this check.
    pub producer: ProducerRef,
    /// The decision that permitted the run, for a command-backed check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_decision: Option<String>,
    /// What actually ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl VerificationCheckResult {
    /// The outcome's own word, for display.
    pub fn outcome_label(&self) -> &'static str {
        match self.outcome {
            CheckOutcome::Passed => "passed",
            CheckOutcome::Failed { .. } => "failed",
            CheckOutcome::Unavailable { .. } => "unavailable",
            CheckOutcome::NotEvaluated { .. } => "not evaluated",
        }
    }

    /// Why it ended that way, when there is more to say than the label.
    pub fn detail(&self) -> Option<&str> {
        match &self.outcome {
            CheckOutcome::Passed => None,
            CheckOutcome::Failed { detail }
            | CheckOutcome::Unavailable { detail }
            | CheckOutcome::NotEvaluated { detail } => Some(detail),
        }
    }

    fn is_required(&self) -> bool {
        matches!(self.requirement, CheckRequirement::Required)
    }
}

/// Aggregate recorded summaries by the same lattice as live results.
///
/// A Change's stored evidence must read back exactly as it was written: the same
/// order of tests, the same refusal to let an optional pass stand in for a
/// required gap, and the same rule that no checks is not a pass.
pub fn aggregate_summaries(
    summaries: &[crate::execution::workspace::CheckResultSummary],
) -> VerificationState {
    let required: Vec<&crate::execution::workspace::CheckResultSummary> = summaries
        .iter()
        .filter(|summary| matches!(summary.requirement, CheckRequirement::Required))
        .collect();

    let of = |state: VerificationStateName| {
        required
            .iter()
            .filter(|summary| summary.state == state)
            .map(|summary| format!("{}: {}", summary.check_id, summary.detail))
            .collect::<Vec<_>>()
    };

    let failures = of(VerificationStateName::Failed);
    if !failures.is_empty() {
        return VerificationState::Failed { failures };
    }
    let unavailable = of(VerificationStateName::Unavailable);
    if !unavailable.is_empty() {
        return VerificationState::Unavailable {
            gaps: vec![crate::extension::CapabilityGap::new(
                crate::extension::ExtensionCapabilityKind::Verification,
                unavailable,
                "a required check had no authorized capability to run",
            )],
        };
    }
    let unevaluated = of(VerificationStateName::NotEvaluated);
    if !unevaluated.is_empty() {
        return VerificationState::NotEvaluated {
            gaps: vec![crate::extension::CapabilityGap::new(
                crate::extension::ExtensionCapabilityKind::Verification,
                unevaluated,
                "a required check did not complete",
            )],
        };
    }
    if required
        .iter()
        .any(|summary| summary.state == VerificationStateName::Passed)
    {
        return VerificationState::Passed;
    }
    VerificationState::NotApplicable {
        reason: "no required check applies to this change".to_string(),
        evidence: summaries
            .iter()
            .map(|summary| summary.check_id.clone())
            .collect(),
    }
}

/// Whether `state` is a legitimate summary of `results`.
///
/// The aggregate lattice is the rule, with exactly one permitted refinement:
/// where the lattice says `NotApplicable` because nothing applied, Draft may
/// record `Unavailable` instead when there was no capability to ask in the
/// first place. Both refuse the gate identically, so the refinement never
/// buys a pass — it only tells the reader that installing something would
/// change the answer. The gap list is that claim's evidence and may not be
/// empty, so an unexplained downgrade is still refused.
pub fn state_matches_results(
    state: &VerificationState,
    results: &[VerificationCheckResult],
) -> bool {
    let derived = aggregate(results);
    if *state == derived {
        return true;
    }
    matches!(
        (state, &derived),
        (
            VerificationState::Unavailable { gaps },
            VerificationState::NotApplicable { .. },
        ) if !gaps.is_empty()
    )
}

/// Aggregate individual outcomes into one state.
///
/// The order of the tests is the whole contract, so it is stated once here:
///
/// 1. a required check failed → `Failed`
/// 2. else a required check had no capability → `Unavailable`
/// 3. else a required check did not complete → `NotEvaluated`
/// 4. else at least one required check ran and passed → `Passed`
/// 5. else nothing applies → `NotApplicable`
///
/// Optional checks are evaluated and reported, but they never decide the
/// aggregate: an optional pass must not mask a required gap, or a project could
/// make its own gate meaningless by adding one cheap optional check.
pub fn aggregate(results: &[VerificationCheckResult]) -> VerificationState {
    let required: Vec<&VerificationCheckResult> = results
        .iter()
        .filter(|result| result.is_required())
        .collect();

    let failures: Vec<String> = required
        .iter()
        .filter_map(|result| match &result.outcome {
            CheckOutcome::Failed { detail } => {
                Some(format!("{}: {detail}", result.check_id.qualified()))
            }
            _ => None,
        })
        .collect();
    if !failures.is_empty() {
        return VerificationState::Failed { failures };
    }

    let unavailable: Vec<&VerificationCheckResult> = required
        .iter()
        .copied()
        .filter(|result| matches!(result.outcome, CheckOutcome::Unavailable { .. }))
        .collect();
    if !unavailable.is_empty() {
        return VerificationState::Unavailable {
            gaps: gaps_for(
                &unavailable,
                "no authorized capability could run this check",
            ),
        };
    }

    let unevaluated: Vec<&VerificationCheckResult> = required
        .iter()
        .copied()
        .filter(|result| matches!(result.outcome, CheckOutcome::NotEvaluated { .. }))
        .collect();
    if !unevaluated.is_empty() {
        return VerificationState::NotEvaluated {
            gaps: gaps_for(&unevaluated, "this check was selected but did not complete"),
        };
    }

    if required
        .iter()
        .any(|result| matches!(result.outcome, CheckOutcome::Passed))
    {
        return VerificationState::Passed;
    }

    VerificationState::NotApplicable {
        reason: "no required verification check applies to this change".into(),
        evidence: results
            .iter()
            .map(|result| {
                format!(
                    "{}: {}",
                    result.check_id.qualified(),
                    outcome_name(&result.outcome)
                )
            })
            .collect(),
    }
}

fn outcome_name(outcome: &CheckOutcome) -> &'static str {
    match outcome {
        CheckOutcome::Passed => "passed",
        CheckOutcome::Failed { .. } => "failed",
        CheckOutcome::Unavailable { .. } => "unavailable",
        CheckOutcome::NotEvaluated { .. } => "not evaluated",
    }
}

fn gaps_for(
    results: &[&VerificationCheckResult],
    reason: &str,
) -> Vec<crate::extension::CapabilityGap> {
    results
        .iter()
        .map(|result| {
            crate::extension::CapabilityGap::new(
                crate::extension::ExtensionCapabilityKind::Verification,
                [result.check_id.qualified()],
                reason,
            )
        })
        .collect()
}

/// Strict project verification configuration (`verify.toml`).
///
/// Project-declared checks sit alongside contributed ones and go through the
/// same runner and the same aggregation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationConfig {
    pub schema_version: u32,
    #[serde(default)]
    pub checks: Vec<ProjectVerificationCheck>,
}

impl crate::contracts::VersionedContract for VerificationConfig {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::VerificationConfig;
}

impl Default for VerificationConfig {
    fn default() -> Self {
        Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::VerificationConfig,
            ),
            checks: Vec::new(),
        }
    }
}

/// A check the project itself declared.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectVerificationCheck {
    pub name: String,
    /// Declared exactly like a contributed one: a program and an argument
    /// vector, run through the same hardened boundary. There is no shell here
    /// either.
    pub command: StructuredCommand,
    #[serde(default = "default_requirement")]
    pub requirement: CheckRequirement,
    #[serde(default)]
    pub enabled: bool,
}

fn default_requirement() -> CheckRequirement {
    CheckRequirement::Required
}

/// Deterministic verification cache key.
///
/// Any component change — observed state, configuration, the environment probes,
/// the checks themselves, the platform, or the exact producer and executable
/// behind a check — deterministically invalidates the key. A matching extension
/// version string is deliberately not enough.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationKey {
    pub snapshot_digest: String,
    pub config_digest: String,
    pub environment_probe_digest: String,
    pub check_selection_digest: String,
    pub producer_digest: String,
    pub platform_digest: String,
    /// Hash over the components above.
    pub key: String,
}

impl VerificationKey {
    pub fn compose(
        snapshot_digest: String,
        config_digest: String,
        environment_probe_digest: String,
        check_selection_digest: String,
        producer_digest: String,
        platform_digest: String,
    ) -> Self {
        let key = hashing::canonical_hash(&serde_json::json!({
            "snapshot_digest": snapshot_digest,
            "config_digest": config_digest,
            "environment_probe_digest": environment_probe_digest,
            "check_selection_digest": check_selection_digest,
            "producer_digest": producer_digest,
            "platform_digest": platform_digest,
            "aggregator_revision": VERIFICATION_AGGREGATOR_REVISION,
        }));
        Self {
            snapshot_digest,
            config_digest,
            environment_probe_digest,
            check_selection_digest,
            producer_digest,
            platform_digest,
            key,
        }
    }
}

/// Canonical digest of the exact producers and executables behind a selection.
///
/// This is what stops a cached result surviving a rebuilt tool: the package may
/// be unchanged while the binary it invokes is not.
pub fn producer_digest(results: &[VerificationCheckResult]) -> String {
    let mut identities: Vec<serde_json::Value> = results
        .iter()
        .map(|result| {
            serde_json::json!({
                "check_id": result.check_id.qualified(),
                "package_digest": result.producer.package_digest,
                "attestation_digest": result.producer.attestation_digest,
                "executable_identity": result.executable_identity,
            })
        })
        .collect();
    identities.sort_by_key(|value| value.to_string());
    hashing::canonical_hash(&identities)
}

/// Canonical digest of the platform an evaluation ran on.
pub fn platform_digest() -> String {
    hashing::canonical_hash(&serde_json::json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "family": std::env::consts::FAMILY,
    }))
}

/// One check Draft decided to run, with everything needed to audit the result
/// whether or not it completes.
///
/// Selection and execution are separate on purpose: a check that could not be
/// run must still appear in the evidence with its requirement intact, or a
/// missing capability would silently shrink the required set.
#[derive(Debug, Clone)]
pub struct SelectedCheck {
    pub check_id: NamespacedId,
    pub display_name: String,
    pub requirement: CheckRequirement,
    pub selection: CheckSelection,
    pub reason: String,
    pub producer: ProducerRef,
    /// What running it requires. `None` means no usable authorized capability
    /// was resolved, and the check is reported `Unavailable`.
    pub command: Option<StructuredCommand>,
    /// The decision that permitted the run, for a command-backed check.
    pub authorization_decision: Option<String>,
    /// Set when several extensions claimed this check id incompatibly. The
    /// check does not run and is reported `NotEvaluated`, so an ambiguity can
    /// never be mistaken for a pass.
    pub ambiguous: bool,
}

/// What running the selected checks produced.
pub struct CheckRun {
    pub results: Vec<VerificationCheckResult>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub duration_ms: u64,
}

/// Run the selected checks and record what each one actually did.
///
/// Every command crosses [`crate::execution::process::run`] — the one execution
/// boundary — so a check inherits its argv-only spawn, cleared environment,
/// timeout and bounded, redacted output. A non-zero exit is a `Failed` result,
/// not an error: a check that reports a problem has done its job.
pub fn run_checks(checks: &[SelectedCheck], working_directory: &Path) -> CheckRun {
    let started = std::time::Instant::now();
    let mut results = Vec::with_capacity(checks.len());
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    for check in checks {
        let (outcome, exit_code, duration_ms, executable_identity) = if check.ambiguous {
            (
                CheckOutcome::NotEvaluated {
                    detail: format!(
                        "several extensions define check '{}' incompatibly",
                        check.check_id.qualified()
                    ),
                },
                None,
                None,
                None,
            )
        } else if let Some(command) = &check.command {
            let limits = crate::execution::process::ProcessLimits {
                timeout_ms: command.timeout_ms.or(Some(DEFAULT_CHECK_TIMEOUT_MS)),
                env_allowlist: Vec::new(),
            };
            match crate::execution::process::run(
                &command.program,
                &command.args,
                working_directory,
                &limits,
            ) {
                Ok(process) => {
                    stdout.extend_from_slice(&process.stdout);
                    stderr.extend_from_slice(&process.stderr);
                    let outcome = if process.timed_out {
                        CheckOutcome::NotEvaluated {
                            detail: format!("'{}' exceeded its time limit", process.display),
                        }
                    } else if process.succeeded() {
                        CheckOutcome::Passed
                    } else {
                        CheckOutcome::Failed {
                            detail: format!("'{}' exited {}", process.display, process.exit_code),
                        }
                    };
                    (
                        outcome,
                        Some(process.exit_code),
                        Some(process.duration_ms),
                        Some(process.display.clone()),
                    )
                }
                // Draft refused to run it, or it could not start. That is a
                // failure to evaluate, never a pass and never a failure of the
                // thing being checked.
                Err(error) => (
                    CheckOutcome::NotEvaluated {
                        detail: error.message.clone(),
                    },
                    None,
                    None,
                    None,
                ),
            }
        } else {
            (
                CheckOutcome::Unavailable {
                    detail: format!(
                        "no authorized capability is installed to run '{}'",
                        check.check_id.qualified()
                    ),
                },
                None,
                None,
                None,
            )
        };

        results.push(VerificationCheckResult {
            check_id: check.check_id.clone(),
            display_name: check.display_name.clone(),
            requirement: check.requirement,
            selection: check.selection,
            reason: check.reason.clone(),
            outcome,
            producer: check.producer.clone(),
            authorization_decision: check.authorization_decision.clone(),
            executable_identity,
            exit_code,
            duration_ms,
        });
    }

    CheckRun {
        results,
        stdout,
        stderr,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

/// The time limit a check inherits when it declares none.
pub const DEFAULT_CHECK_TIMEOUT_MS: u64 = 600_000;

/// The namespace a project's own configured checks are minted under.
pub const PROJECT_CHECK_NAMESPACE: &str = "draft.project";

/// The identifier for a check the project configured by name.
///
/// Project checks live in their own namespace so they can never collide with a
/// contributed one, and so evidence always says which is which.
pub fn project_check_id(name: &str) -> DraftResult<NamespacedId> {
    let slug: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        return Err(DraftError::invalid_config(format!(
            "verification check name '{name}' has no usable identifier"
        )));
    }
    NamespacedId::parse(&format!("{PROJECT_CHECK_NAMESPACE}/{slug}")).map_err(|error| {
        DraftError::invalid_config(format!(
            "verification check name '{name}' is not a valid identifier: {error}"
        ))
    })
}

/// The producer record for a check the project configured itself.
///
/// Configuration is the project's own voice, so it is attributed to Draft
/// rather than to an invented publisher — but it still carries a producer, so
/// every result in the evidence can be traced to something.
pub fn project_producer() -> ProducerRef {
    ProducerRef {
        extension_id: "draft.core".to_string(),
        extension_version: crate::DRAFT_VERSION.to_string(),
        package_digest: String::new(),
        attestation_digest: String::new(),
    }
}

/// The verification evidence persisted alongside a Change.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyEvidence {
    pub schema_version: u32,
    pub change_id: String,
    pub revision_id: String,
    pub revision_digest: String,
    pub dependency_digests: Vec<String>,
    /// Exactly what this evidence was derived from.
    pub inputs: DerivationInputs,
    /// The Core semantics that produced the aggregate state.
    pub aggregator_revision: u32,
    pub check_results: Vec<VerificationCheckResult>,
    pub state: VerificationState,
    pub selection_reason: String,
    pub duration_ms: u64,
    pub stdout_digest: String,
    pub stderr_digest: String,
    pub result_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_key: Option<VerificationKey>,
}

impl crate::contracts::VersionedContract for VerifyEvidence {
    const CONTRACT: crate::contracts::ContractId =
        crate::contracts::ContractId::VerificationEvidence;
}

impl VerifyEvidence {
    pub fn validate_binding(&self, revision: &RevisionRecord) -> DraftResult<()> {
        if !crate::contracts::supports_version(
            crate::contracts::ContractId::VerificationEvidence,
            self.schema_version,
        ) {
            return Err(DraftError::new(
                DraftErrorKind::UnsupportedSchema,
                format!(
                    "verification evidence schema {} is unsupported",
                    self.schema_version
                ),
            ));
        }
        if self.change_id != revision.change_id
            || self.revision_id != revision.revision_id
            || self.revision_digest != revision.revision_digest
            || self.dependency_digests != revision.resolved_dependency_digests
        {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                "verification evidence is bound to a different Change revision or dependencies",
            ));
        }
        if self.result_hash != self.compute_result_hash() {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                "verification evidence result digest mismatch",
            ));
        }
        // The stored state must be the one the recorded results imply, so a
        // tampered or stale summary cannot claim a pass the checks do not show.
        if !state_matches_results(&self.state, &self.check_results) {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                "verification state does not match the recorded check results",
            ));
        }
        Ok(())
    }

    /// Recompute the result hash over the evidence (excluding the hash itself).
    pub fn compute_result_hash(&self) -> String {
        hashing::canonical_hash(&serde_json::json!({
            "schema_version": self.schema_version,
            "change_id": self.change_id,
            "revision_id": self.revision_id,
            "revision_digest": self.revision_digest,
            "dependency_digests": self.dependency_digests,
            "inputs": self.inputs,
            "aggregator_revision": self.aggregator_revision,
            "check_results": self.check_results,
            "state": self.state,
            "selection_reason": self.selection_reason,
            "stdout_digest": self.stdout_digest,
            "stderr_digest": self.stderr_digest,
        }))
    }

    /// Whether this evidence satisfies a gate on its own.
    pub fn satisfies_gate_unconditionally(&self) -> bool {
        self.state.satisfies_gate_unconditionally()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> NamespacedId {
        NamespacedId::parse(value).unwrap()
    }

    fn producer(extension: &str) -> ProducerRef {
        ProducerRef {
            extension_id: extension.into(),
            extension_version: "1.0.0".into(),
            package_digest: format!("sha256:{extension}"),
            attestation_digest: format!("sha256:att-{extension}"),
        }
    }

    fn result(
        check_id: &str,
        requirement: CheckRequirement,
        outcome: CheckOutcome,
    ) -> VerificationCheckResult {
        VerificationCheckResult {
            check_id: id(check_id),
            display_name: check_id.into(),
            requirement,
            selection: CheckSelection::Whole,
            reason: "selected".into(),
            outcome,
            producer: producer("ex.pub"),
            authorization_decision: Some("sha256:decision".into()),
            executable_identity: Some("sha256:binary".into()),
            exit_code: Some(0),
            duration_ms: Some(10),
        }
    }

    #[test]
    fn no_checks_can_never_mean_passed() {
        // The failure this whole model exists to prevent: an empty selection
        // reporting success and satisfying a submission gate.
        let state = aggregate(&[]);
        assert!(matches!(state, VerificationState::NotApplicable { .. }));
        assert!(!state.satisfies_gate_unconditionally());
    }

    #[test]
    fn nothing_installed_is_recorded_as_unavailable_and_still_validates() {
        // With no verification capability there are no checks, so the lattice
        // says `NotApplicable`. Draft records the more informative
        // `Unavailable` instead, and evidence written that way must still load.
        let derived = aggregate(&[]);
        assert!(matches!(derived, VerificationState::NotApplicable { .. }));

        let recorded = VerificationState::Unavailable {
            gaps: vec![crate::extension::CapabilityGap::new(
                crate::extension::ExtensionCapabilityKind::Verification,
                Vec::<String>::new(),
                "no verification capability is installed",
            )],
        };
        assert!(state_matches_results(&recorded, &[]));
        assert!(!recorded.satisfies_gate_unconditionally());

        // An `Unavailable` with nothing to point at is an unexplained
        // downgrade, and is refused exactly like a tampered summary.
        assert!(!state_matches_results(
            &VerificationState::Unavailable { gaps: vec![] },
            &[]
        ));
        assert!(!state_matches_results(&VerificationState::Passed, &[]));
    }

    #[test]
    fn the_five_states_are_distinct_and_only_passed_satisfies_a_gate() {
        let states = [
            VerificationState::Passed,
            VerificationState::Failed {
                failures: vec!["x".into()],
            },
            VerificationState::Unavailable { gaps: vec![] },
            VerificationState::NotEvaluated { gaps: vec![] },
            VerificationState::NotApplicable {
                reason: "none apply".into(),
                evidence: vec![],
            },
        ];
        let names: Vec<&str> = states.iter().map(VerificationState::as_str).collect();
        assert_eq!(
            names,
            [
                "passed",
                "failed",
                "unavailable",
                "not_evaluated",
                "not_applicable"
            ]
        );
        for state in &states {
            assert_eq!(
                state.satisfies_gate_unconditionally(),
                matches!(state, VerificationState::Passed)
            );
        }
    }

    #[test]
    fn the_lattice_is_ordered_failure_first() {
        let failed = result(
            "ex.pub/a",
            CheckRequirement::Required,
            CheckOutcome::Failed {
                detail: "assertion".into(),
            },
        );
        let unavailable = result(
            "ex.pub/b",
            CheckRequirement::Required,
            CheckOutcome::Unavailable {
                detail: "no capability".into(),
            },
        );
        let unevaluated = result(
            "ex.pub/c",
            CheckRequirement::Required,
            CheckOutcome::NotEvaluated {
                detail: "timed out".into(),
            },
        );
        let passed = result("ex.pub/d", CheckRequirement::Required, CheckOutcome::Passed);

        // A failure outranks everything, then unavailability, then
        // non-evaluation. A pass only wins when nothing worse happened.
        assert!(matches!(
            aggregate(&[
                passed.clone(),
                unevaluated.clone(),
                unavailable.clone(),
                failed.clone()
            ]),
            VerificationState::Failed { .. }
        ));
        assert!(matches!(
            aggregate(&[passed.clone(), unevaluated.clone(), unavailable]),
            VerificationState::Unavailable { .. }
        ));
        assert!(matches!(
            aggregate(&[passed.clone(), unevaluated]),
            VerificationState::NotEvaluated { .. }
        ));
        assert_eq!(aggregate(&[passed]), VerificationState::Passed);
    }

    #[test]
    fn an_optional_pass_never_masks_a_required_gap() {
        // Otherwise a project could neutralise its own gate by adding one cheap
        // optional check that always succeeds.
        let optional_pass = result(
            "ex.pub/opt",
            CheckRequirement::Optional,
            CheckOutcome::Passed,
        );
        let required_gap = result(
            "ex.pub/req",
            CheckRequirement::Required,
            CheckOutcome::Unavailable {
                detail: "not installed".into(),
            },
        );
        assert!(matches!(
            aggregate(&[optional_pass.clone(), required_gap]),
            VerificationState::Unavailable { .. }
        ));

        // And an optional check alone does not amount to a pass.
        assert!(matches!(
            aggregate(&[optional_pass]),
            VerificationState::NotApplicable { .. }
        ));
    }

    #[test]
    fn a_gap_names_the_check_but_no_package() {
        let state = aggregate(&[result(
            "ex.pub/req",
            CheckRequirement::Required,
            CheckOutcome::Unavailable {
                detail: "not installed".into(),
            },
        )]);
        match state {
            VerificationState::Unavailable { gaps } => {
                assert_eq!(gaps[0].resource_classes, vec!["ex.pub/req".to_string()]);
                let encoded = serde_json::to_value(&gaps[0]).unwrap();
                assert!(encoded.get("extension_id").is_none());
                assert!(encoded.get("package").is_none());
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn the_cache_key_distinguishes_a_rebuilt_executable() {
        let with_binary = |digest: &str| {
            let mut check = result("ex.pub/a", CheckRequirement::Required, CheckOutcome::Passed);
            check.executable_identity = Some(digest.into());
            producer_digest(std::slice::from_ref(&check))
        };
        // The package is unchanged; the binary it invokes is not.
        assert_ne!(with_binary("sha256:one"), with_binary("sha256:two"));
        assert_eq!(with_binary("sha256:one"), with_binary("sha256:one"));
    }

    #[test]
    fn evidence_cannot_claim_a_state_its_results_do_not_show() {
        let results = vec![result(
            "ex.pub/a",
            CheckRequirement::Required,
            CheckOutcome::Failed {
                detail: "assertion".into(),
            },
        )];
        let mut evidence = VerifyEvidence {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::VerificationEvidence,
            ),
            change_id: "chg_1".into(),
            revision_id: "rev_1".into(),
            revision_digest: "sha256:rev".into(),
            dependency_digests: vec![],
            inputs: DerivationInputs::new(
                crate::provenance::derived::SubjectRef::ChangeSet {
                    change_set_digest: "sha256:change".into(),
                },
                [],
            ),
            aggregator_revision: VERIFICATION_AGGREGATOR_REVISION,
            check_results: results.clone(),
            // A tampered summary claiming success.
            state: VerificationState::Passed,
            selection_reason: "all".into(),
            duration_ms: 1,
            stdout_digest: "sha256:out".into(),
            stderr_digest: "sha256:err".into(),
            result_hash: String::new(),
            verification_key: None,
        };
        evidence.result_hash = evidence.compute_result_hash();
        let revision = RevisionRecord {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::RevisionRecord,
            ),
            change_id: "chg_1".into(),
            manifest_digest: "sha256:manifest".into(),
            revision_id: "rev_1".into(),
            revision_number: 1,
            revision_digest: "sha256:rev".into(),
            base_digest: String::new(),
            content_digest: String::new(),
            change_digest: String::new(),
            target_digest: String::new(),
            resolved_dependency_digests: vec![],
            created_at: String::new(),
        };
        let error = evidence.validate_binding(&revision).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);

        evidence.state = aggregate(&results);
        evidence.result_hash = evidence.compute_result_hash();
        evidence.validate_binding(&revision).unwrap();
    }

    #[test]
    fn a_project_declared_check_has_no_shell_either() {
        let check = ProjectVerificationCheck {
            name: "smoke".into(),
            command: StructuredCommand {
                program: "example-verify".into(),
                args: vec!["--all".into()],
                cwd: None,
                timeout_ms: None,
            },
            requirement: CheckRequirement::Required,
            enabled: true,
        };
        check.command.validate().unwrap();
        // The same refusal a contributed command gets.
        let shelled = ProjectVerificationCheck {
            command: StructuredCommand {
                program: "sh -c".into(),
                args: vec!["rm -rf /".into()],
                cwd: None,
                timeout_ms: None,
            },
            ..check
        };
        assert!(shelled.command.validate().is_err());
    }
}
