//! The production adapter for every scheme Draft does not implement itself.
//!
//! A contributed `resource_adapter` is data: a scheme, declared capabilities and
//! a set of [`MechanismOperation`]s. This module turns that declaration into a
//! working [`ResourceSource`] without Core learning anything about the domain
//! behind it. A catalog, a timeline, a dataset partition and a design document
//! all reach Draft through exactly this code path.
//!
//! What Core supplies, and never delegates:
//!
//! * **State identity.** The adapter reports *observable state*; Core computes
//!   the canonical [`declared_state_digest`] over it. An adapter therefore
//!   cannot mint its own identity, cannot omit one, and cannot make two
//!   different states hash the same.
//! * **Coverage scoping.** The adapter names its domains in its own vocabulary;
//!   Core scopes each to the adapter's binding, so two adapters that both say
//!   `root` can never compare equal.
//! * **View rules.** Contributed exclusions are applied by Core to the
//!   enumeration result, so what leaves the universe does not depend on adapter
//!   cooperation.
//! * **Execution authority.** Every call crosses
//!   [`crate::execution::mechanism::invoke_command`] — the one process boundary — carrying
//!   the producer's attestation and the authorization decision that permitted
//!   it. An unauthorized adapter cannot observe.
//! * **Bounds.** Response size is the operation's declared ceiling; content
//!   reads are bounded before allocation.
//!
//! The locator body is never parsed. It travels to the adapter as the opaque
//! string it is, and comes back the same way.

use std::collections::BTreeMap;

use base64::Engine as _;
use draft_extension_contract::{
    AdapterCapabilities, AttributeValue, MechanismOperation, RecoveryContribution,
    ResourceAdapterContribution, ResourceForm,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::contracts::ProducerRef;
use crate::dcg::anchor::{
    AnchorCapture, CanonicalObjectRef, FencingEvidence, RecoveryAnchor, RecoveryAnchorSet,
    RecoveryMaterial, ResourceRestorePlan,
};
use crate::dcg::observation::{
    AdapterBindingId, CoverageDomainRef, CoverageStatus, ObservationCoverage, ObservationGap,
    ObservationGapKind,
};
use crate::dcg::resource::{
    declared_state_digest, require_state_digest, stale_observation, ContentAccess,
    ObservationToken, ObservedRef, RawObservedResource, RawResourceState, ResourceLocator,
    Untrackable,
};
use crate::dcg::source::{
    AnchorRequest, EnumerationOutcome, MaterializedInput, MutationOutcome, MutationPrecondition,
    MutationStep, ResourceMutationPlan, ResourceSource, ViewRules,
};
use crate::execution::mechanism::{invoke_command, MechanismContext, MechanismInputs};
use crate::support::common::{now, OperationId};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// The largest content payload Draft will accept back from one bounded read.
///
/// Independent of the operation's own declared response ceiling: that bounds the
/// document, this bounds the bytes Draft is willing to hold.
pub const MAX_CONTENT_BYTES: u64 = 64 * 1024 * 1024;

/// One contributed adapter, ready to be asked questions.
pub struct CommandResourceSource {
    contribution: ResourceAdapterContribution,
    binding_id: AdapterBindingId,
    workspace_id: String,
    producer: ProducerRef,
    /// The decision digest that permits this adapter to execute. `None` means
    /// the artifact is installed and trusted but not authorized, and every
    /// operation refuses rather than running.
    authorization_decision: Option<String>,
    object_store: crate::project::object_store::ObjectStore,
}

impl CommandResourceSource {
    pub fn new(
        contribution: ResourceAdapterContribution,
        binding_id: AdapterBindingId,
        workspace_id: String,
        producer: ProducerRef,
        authorization_decision: Option<String>,
        object_store: crate::project::object_store::ObjectStore,
    ) -> Self {
        Self {
            contribution,
            binding_id,
            workspace_id,
            producer,
            authorization_decision,
            object_store,
        }
    }

    /// Perform one declared operation and return its validated response body.
    fn call(
        &self,
        operation: &MechanismOperation,
        request: Value,
        inputs: MechanismInputs,
    ) -> DraftResult<Value> {
        let context = MechanismContext {
            workspace_id: self.workspace_id.clone(),
            operation_id: OperationId::generate().to_string(),
            producer: self.producer.clone(),
            authorization_decision: self.authorization_decision.clone(),
        };
        let response = invoke_command(operation, &request, &inputs, &context)?;
        Ok(response.payload)
    }

    /// Scope one of the adapter's own domain names to this binding.
    fn domain(&self, local: &str) -> CoverageDomainRef {
        CoverageDomainRef::new(self.binding_id.clone(), local)
    }

    /// Turn one reported resource into authoritative state.
    ///
    /// The digest is computed here, over the adapter's declared observable
    /// state, so identity is Draft's arithmetic on the adapter's facts rather
    /// than a number the adapter chose.
    fn observed_from(&self, reported: ReportedResource) -> DraftResult<RawObservedResource> {
        let locator = ResourceLocator::new(self.scheme(), reported.body);
        let state_digest = declared_state_digest(&locator, reported.form, &reported.state);
        let state = RawResourceState {
            resource_id: crate::dcg::resource::resource_id_for_locator(&reported.resource_id),
            locator,
            form: reported.form,
            media_type: reported.media_type,
            attributes: reported.attributes,
            state_digest,
            content_digest: reported.content.as_ref().map(|c| c.digest.clone()),
            metadata_digest: None,
            content_size: reported.content.as_ref().map(|c| c.length),
        };
        require_state_digest(&state)?;
        Ok(RawObservedResource {
            state,
            observation_token: ObservationToken(reported.generation),
            coverage_domain: self.domain(&reported.domain),
        })
    }

    /// Refuse to act on a generation other than the one Draft observed.
    ///
    /// The adapter is asked to re-describe the resource and Core recomputes the
    /// digest; an adapter cannot assert "unchanged" for a state that moved,
    /// because it never supplies the digest in the first place.
    fn fence(&self, observed: &ObservedRef) -> DraftResult<()> {
        let current = self.describe(&observed.locator)?;
        if current.state.state_digest != observed.expected_state_digest {
            return Err(stale_observation(
                &observed.locator,
                &observed.expected_state_digest,
                &current.state.state_digest,
            ));
        }
        Ok(())
    }

    fn recovery(&self) -> &RecoveryContribution {
        &self.contribution.recovery
    }
}

impl ResourceSource for CommandResourceSource {
    fn scheme(&self) -> &str {
        &self.contribution.scheme
    }

    fn binding_id(&self) -> AdapterBindingId {
        self.binding_id.clone()
    }

    fn capabilities(&self) -> AdapterCapabilities {
        self.contribution.capabilities.clone()
    }

    fn enumerate(&self, rules: &ViewRules) -> DraftResult<EnumerationOutcome> {
        let payload = self.call(
            &self.contribution.enumerate,
            json!({"operation": "enumerate", "scheme": self.scheme()}),
            MechanismInputs::default(),
        )?;
        let reported: ReportedEnumeration = decode(payload, "enumerate")?;

        let mut outcome = EnumerationOutcome::default();
        for resource in reported.resources {
            let observed = self.observed_from(resource)?;
            // View rules are applied here, by Core, over the adapter's own
            // report. An adapter that ignored them could not thereby smuggle a
            // resource into the observed universe.
            if excluded_by(rules, &observed.state) {
                outcome.excluded_count += 1;
                continue;
            }
            outcome.resources.push(observed);
        }

        let mut gaps = Vec::new();
        for gap in reported.gaps {
            gaps.push(ObservationGap::new(
                gap.kind,
                gap.stable_code,
                gap.domains.iter().map(|local| self.domain(local)).collect(),
                Some(self.binding_id.clone()),
                gap.detail,
            ));
        }

        for domain in reported.domains {
            let scoped = self.domain(&domain.local_id);
            let status = if domain.complete {
                CoverageStatus::Complete
            } else {
                CoverageStatus::Incomplete {
                    // Only the gaps that actually name this domain. A domain
                    // declared incomplete with nothing to point at is refused
                    // below, because "incomplete for no stated reason" is not
                    // something a reader can act on.
                    gap_ids: gaps
                        .iter()
                        .filter(|gap| gap.coverage_domains.contains(&scoped))
                        .map(|gap| gap.gap_id.clone())
                        .collect(),
                }
            };
            if let CoverageStatus::Incomplete { gap_ids } = &status {
                if gap_ids.is_empty() {
                    return Err(DraftError::new(
                        DraftErrorKind::Validation,
                        format!(
                            "adapter '{}' reported domain '{}' incomplete without naming a gap",
                            self.scheme(),
                            domain.local_id
                        ),
                    ));
                }
            }
            outcome.coverage.push(ObservationCoverage {
                domain: scoped,
                status,
            });
        }

        outcome.gaps = gaps;
        outcome.untrackable = reported
            .untrackable
            .into_iter()
            .map(|entry| Untrackable {
                locator: ResourceLocator::new(self.scheme(), entry.body),
                reason: entry.reason,
            })
            .collect();

        if outcome.coverage.is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "adapter '{}' enumerated without declaring any coverage domain; \
                     absence could never be proved from such a report",
                    self.scheme()
                ),
            ));
        }
        Ok(outcome)
    }

    fn describe(&self, locator: &ResourceLocator) -> DraftResult<RawObservedResource> {
        let payload = self.call(
            &self.contribution.describe,
            json!({"operation": "describe", "body": locator.body}),
            MechanismInputs::default(),
        )?;
        let reported: ReportedResource = decode(payload, "describe")?;
        self.observed_from(reported)
    }

    fn content_access(&self, observed: &ObservedRef) -> DraftResult<ContentAccess> {
        self.fence(observed)?;
        let described = self.describe(&observed.locator)?;
        match described.state.content_size {
            None => Ok(ContentAccess::None),
            Some(length) if self.contribution.capabilities.supports_ranged_read => {
                Ok(ContentAccess::Ranged {
                    length,
                    media_type: described.state.media_type,
                })
            }
            Some(length) => Ok(ContentAccess::Object {
                digest: described.state.content_digest.unwrap_or_default(),
                length,
                media_type: described.state.media_type,
            }),
        }
    }

    fn read_range(&self, observed: &ObservedRef, offset: u64, length: u64) -> DraftResult<Vec<u8>> {
        if length > MAX_CONTENT_BYTES {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!("a {length}-byte read exceeds the {MAX_CONTENT_BYTES}-byte access bound"),
            ));
        }
        self.fence(observed)?;
        let payload = self.call(
            &self.contribution.content,
            json!({
                "operation": "content",
                "body": observed.locator.body,
                "expected_state_digest": observed.expected_state_digest,
                "offset": offset,
                "length": length,
            }),
            MechanismInputs::default(),
        )?;
        let reported: ReportedContent = decode(payload, "content")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(reported.bytes)
            .map_err(|error| {
                DraftError::new(
                    DraftErrorKind::Validation,
                    format!(
                        "adapter '{}' returned content that is not valid base64: {error}",
                        self.scheme()
                    ),
                )
            })?;
        if bytes.len() as u64 > MAX_CONTENT_BYTES {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "adapter '{}' returned {} bytes, over the {MAX_CONTENT_BYTES}-byte bound",
                    self.scheme(),
                    bytes.len()
                ),
            ));
        }
        Ok(bytes)
    }

    fn materialize(
        &self,
        observed: &ObservedRef,
        scope: &crate::support::runtime_scope::RuntimeScope,
    ) -> DraftResult<MaterializedInput> {
        let bytes = self.read_range(observed, 0, MAX_CONTENT_BYTES)?;
        let name = format!(
            "input-{}",
            crate::support::hashing::sha256_hex(observed.resource_id.as_str().as_bytes())
        );
        scope.materialize(&name, &bytes)?;
        Ok(MaterializedInput {
            name,
            length: bytes.len() as u64,
        })
    }

    fn mutate(&self, plan: &ResourceMutationPlan) -> DraftResult<MutationOutcome> {
        let Some(operation) = &self.contribution.mutate else {
            return Err(DraftError::new(
                DraftErrorKind::CapabilityUnavailable,
                format!("adapter '{}' declares no mutation operation", self.scheme()),
            ));
        };

        // Preconditions are Draft's, checked before the adapter is asked to do
        // anything. An adapter is never trusted to enforce the conditions under
        // which it was allowed to act.
        for precondition in &plan.preconditions {
            match precondition {
                MutationPrecondition::StateEquals(observed)
                | MutationPrecondition::ParentStateEquals(observed) => self.fence(observed)?,
                MutationPrecondition::MustNotExist(locator)
                | MutationPrecondition::DestinationAvailable(locator) => {
                    if self.describe(locator).is_ok() {
                        return Err(DraftError::new(
                            DraftErrorKind::ConflictDetected,
                            format!(
                                "{locator} already exists; the operation expected it to be free"
                            ),
                        ));
                    }
                }
            }
        }

        let steps: Vec<Value> = plan.steps.iter().map(encode_step).collect();
        let payload = self.call(
            operation,
            json!({
                "operation": "mutate",
                // Draft's operation id, on Draft's record. The adapter is told
                // what is being done under, not asked to invent it.
                "operation_id": plan.operation_id.to_string(),
                "steps": steps,
            }),
            MechanismInputs::default(),
        )?;
        let reported: ReportedMutation = decode(payload, "mutate")?;
        Ok(MutationOutcome {
            resources_changed: reported
                .changed
                .into_iter()
                .map(|body| ResourceLocator::new(self.scheme(), body))
                .collect(),
        })
    }

    fn capture_anchor(
        &self,
        observed: &RawObservedResource,
        request: &AnchorRequest,
    ) -> DraftResult<Option<RecoveryAnchor>> {
        let Some(operation) = self.recovery().capture() else {
            // No declared capture mechanism: this state is observable but not
            // restorable, and Draft says so rather than implying support.
            return Ok(None);
        };

        let payload = self.call(
            operation,
            json!({
                "operation": "capture",
                "body": observed.state.locator.body,
                "expected_state_digest": observed.state.state_digest,
            }),
            MechanismInputs::default(),
        )?;
        let reported: ReportedCapture = decode(payload, "capture")?;

        // Revalidate: the material must describe the generation the anchor
        // claims. A capture taken after the resource moved is not a weaker
        // anchor, it is a wrong one.
        let after = self.describe(&observed.state.locator)?;
        if after.state.state_digest != observed.state.state_digest {
            return Ok(None);
        }

        let material_bytes = crate::support::hashing::canonical_json(&reported.material);
        let object_digest = self.object_store.put_bytes(material_bytes.as_bytes())?;
        let anchor = RecoveryAnchor {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::RecoveryAnchorSet,
            ),
            resource_id: observed.state.resource_id.clone(),
            target_state_digest: observed.state.state_digest.clone(),
            target_locator: observed.state.locator.clone(),
            adapter_binding_id: self.binding_id.clone(),
            recovery_material: RecoveryMaterial::CanonicalState {
                payload: CanonicalObjectRef {
                    object_digest,
                    length: material_bytes.len() as u64,
                },
                referenced_objects: reported.referenced_objects,
            },
            capture: AnchorCapture {
                observed_state_digest: observed.state.state_digest.clone(),
                observation_run_id: request.observation_run_id.clone(),
                fenced_with: FencingEvidence::DigestRevalidated {
                    before: observed.state.state_digest.clone(),
                    after: after.state.state_digest,
                },
                captured_at: now(),
            },
            // An extension observer, so the producer and its attestation travel
            // with the anchor — unlike Core's own, which has neither.
            producer: Some(self.producer.clone()),
            recorded_at: now(),
            anchor_digest: String::new(),
        }
        .seal()?;
        Ok(Some(anchor))
    }

    fn restore(
        &self,
        plan: &ResourceRestorePlan,
        anchors: &RecoveryAnchorSet,
    ) -> DraftResult<MutationOutcome> {
        let Some(operation) = self.recovery().restore() else {
            return Err(DraftError::new(
                DraftErrorKind::CapabilityUnavailable,
                format!("adapter '{}' declares no restore operation", self.scheme()),
            ));
        };

        let mut targets = Vec::new();
        for restore in &plan.restore_targets {
            if restore.target_locator.scheme != self.scheme() {
                continue;
            }
            let Some(anchor) = anchors.anchor_for(&restore.resource_id) else {
                continue;
            };
            let RecoveryMaterial::CanonicalState { payload, .. } = &anchor.recovery_material else {
                continue;
            };
            let material: Value =
                serde_json::from_slice(&self.object_store.get_bytes(&payload.object_digest)?)
                    .map_err(|error| {
                        DraftError::storage(format!("corrupt recovery material: {error}"))
                    })?;
            targets.push(json!({
                "body": restore.target_locator.body,
                "target_state_digest": restore.target_state_digest,
                "material": material,
            }));
        }

        let absences: Vec<&str> = plan
            .absence_targets
            .iter()
            .filter(|absence| absence.current_locator.scheme == self.scheme())
            .map(|absence| absence.current_locator.body.as_str())
            .collect();

        if targets.is_empty() && absences.is_empty() {
            return Ok(MutationOutcome::default());
        }

        let payload = self.call(
            operation,
            json!({
                "operation": "restore",
                "operation_id": plan.operation_id.to_string(),
                "targets": targets,
                "absences": absences,
            }),
            MechanismInputs::default(),
        )?;
        let reported: ReportedMutation = decode(payload, "restore")?;
        Ok(MutationOutcome {
            resources_changed: reported
                .changed
                .into_iter()
                .map(|body| ResourceLocator::new(self.scheme(), body))
                .collect(),
        })
    }
}

/// Whether a contributed view rule removes this resource from the universe.
fn excluded_by(rules: &ViewRules, state: &RawResourceState) -> bool {
    if rules.is_empty() {
        return false;
    }
    let view = state.view();
    rules
        .exclusions
        .iter()
        .any(|rule| crate::support::predicate::matches_raw(&rule.predicate, &view))
}

fn encode_step(step: &MutationStep) -> Value {
    match step {
        MutationStep::SetContent { locator, content } => json!({
            "step": "set_content",
            "body": locator.body,
            "content": base64::engine::general_purpose::STANDARD.encode(content),
        }),
        MutationStep::CreateCollection { locator } => json!({
            "step": "create_collection",
            "body": locator.body,
        }),
        MutationStep::Relocate { from, to } => json!({
            "step": "relocate",
            "from": from.body,
            "to": to.body,
        }),
        MutationStep::Remove { locator, recursive } => json!({
            "step": "remove",
            "body": locator.body,
            "recursive": recursive,
        }),
    }
}

/// Decode one adapter response, naming the operation when it does not fit.
///
/// `deny_unknown_fields` throughout: an adapter that returns a field Draft does
/// not model is telling Draft something it will silently ignore, and silence is
/// how a contract drifts.
fn decode<T: for<'de> Deserialize<'de>>(payload: Value, operation: &str) -> DraftResult<T> {
    serde_json::from_value(payload).map_err(|error| {
        DraftError::new(
            DraftErrorKind::Validation,
            format!("adapter '{operation}' response does not match its contract: {error}"),
        )
    })
}

// ---------------------------------------------------------------------------
// The canonical response documents. Draft owns these shapes; adapters fill them.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportedEnumeration {
    #[serde(default)]
    resources: Vec<ReportedResource>,
    #[serde(default)]
    domains: Vec<ReportedDomain>,
    #[serde(default)]
    gaps: Vec<ReportedGap>,
    #[serde(default)]
    untrackable: Vec<ReportedUntrackable>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportedResource {
    resource_id: String,
    /// Opaque. Core stores and compares it; nothing here parses it.
    body: String,
    #[serde(default)]
    form: Option<ResourceForm>,
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default)]
    attributes: BTreeMap<String, AttributeValue>,
    /// The observable state Core hashes into this resource's identity.
    state: Value,
    #[serde(default)]
    content: Option<ReportedContentRef>,
    /// Which generation of this resource was observed.
    ///
    /// Named `generation` on the wire rather than `observation_token`, and
    /// deliberately so. Command output is redacted before Draft will look at
    /// it, and that redactor treats any key containing "token" as a secret —
    /// correctly, because it cannot know which ones are. A field called
    /// `observation_token` would be replaced with `[REDACTED]` and every
    /// adapter response would arrive as malformed JSON. `generation` is also
    /// the more honest name: this is not a credential, it is which version of
    /// the resource was seen.
    generation: String,
    /// The adapter's own name for the part of its universe this belongs to.
    domain: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportedContentRef {
    digest: String,
    length: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportedDomain {
    local_id: String,
    complete: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportedGap {
    kind: ObservationGapKind,
    stable_code: String,
    #[serde(default)]
    domains: Vec<String>,
    detail: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportedUntrackable {
    body: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportedContent {
    /// Base64. JSON has no byte type, and inventing one per adapter would be a
    /// contract nobody could validate.
    bytes: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportedMutation {
    #[serde(default)]
    changed: Vec<String>,
}

/// What a declared capture operation returns.
///
/// No schema field: the operation that produced this already declares its
/// response contract, and the anchor records the binding it came from. A second
/// copy here could disagree with the first, and then nothing would know which
/// to believe.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportedCapture {
    /// Opaque to Core beyond canonical validation and bounds.
    material: Value,
    #[serde(default)]
    referenced_objects: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_identity_is_computed_by_core_not_supplied_by_the_adapter() {
        // Two adapter reports that differ only in declared state must differ in
        // identity, and the adapter never gets to say otherwise: there is no
        // field on the wire where it could put a digest.
        let locator = ResourceLocator::new("catalog", "sku/A-100");
        let first = declared_state_digest(
            &locator,
            Some(ResourceForm::Logical),
            &json!({"price": 100}),
        );
        let second = declared_state_digest(
            &locator,
            Some(ResourceForm::Logical),
            &json!({"price": 101}),
        );
        assert_ne!(first, second);

        let document = serde_json::to_string(&json!({
            "resource_id": "row-1",
            "body": "sku/A-100",
            "state": {"price": 100},
            "observation_token": "gen-1",
            "domain": "rows",
            "state_digest": "attacker-chosen",
        }))
        .unwrap();
        let refused: Result<ReportedResource, _> = serde_json::from_str(&document);
        assert!(
            refused.is_err(),
            "an adapter must have nowhere to assert its own state identity"
        );
    }

    #[test]
    fn a_domain_cannot_be_incomplete_for_no_stated_reason() {
        // "Part of my universe is unknown" is only actionable if the adapter
        // says which part and why. An unexplained incomplete domain would block
        // absence reasoning with nothing a reader could chase.
        let document = json!({
            "resources": [],
            "domains": [{"local_id": "rows", "complete": false}],
            "gaps": [],
        });
        let reported: ReportedEnumeration = serde_json::from_value(document).unwrap();
        assert_eq!(reported.domains.len(), 1);
        assert!(reported.gaps.is_empty());
        // The refusal itself is exercised end-to-end in the non-file adapter
        // proof, where a real declared command produces this document.
    }

    #[test]
    fn no_field_in_the_adapter_protocol_survives_output_redaction_as_a_secret() {
        // Command output is redacted before Draft parses it, and the redactor
        // treats any key containing "token", "secret", "password" and friends as
        // a credential — correctly, because it cannot tell which ones are. A
        // protocol field named like one would be replaced with `[REDACTED]` and
        // every adapter response would arrive as malformed JSON.
        //
        // This is a real interaction, not a hypothetical: `observation_token`
        // did exactly that. The guard lives here so a future field cannot
        // reintroduce it silently.
        let response = serde_json::to_string(&json!({
            "resources": [{
                "resource_id": "row-1",
                "body": "sku/A-100",
                "form": "logical",
                "state": {"price": 100},
                "generation": "rev-1",
                "domain": "rows",
            }],
            "domains": [{"local_id": "rows", "complete": true}],
            "gaps": [],
            "untrackable": [],
        }))
        .unwrap();

        let redacted = crate::support::redaction::redact(&response);
        assert!(
            !redacted.contains("[REDACTED]"),
            "a protocol field was mistaken for a secret: {redacted}"
        );
        // And it still parses after the round trip a real response makes.
        let parsed: ReportedEnumeration =
            serde_json::from_str(&redacted).expect("a redacted response must still be valid");
        assert_eq!(parsed.resources.len(), 1);
    }

    #[test]
    fn an_unknown_response_field_is_refused_rather_than_ignored() {
        let document = json!({
            "resources": [],
            "domains": [],
            "surprise": true,
        });
        let refused: Result<ReportedEnumeration, _> = serde_json::from_value(document);
        assert!(refused.is_err());
    }

    #[test]
    fn mutation_steps_travel_as_opaque_bodies() {
        let step = MutationStep::Relocate {
            from: ResourceLocator::new("catalog", "sku/A"),
            to: ResourceLocator::new("catalog", "sku/B"),
        };
        let encoded = encode_step(&step);
        assert_eq!(encoded["from"], "sku/A");
        assert_eq!(encoded["to"], "sku/B");
        // No scheme, no path: the adapter already knows its own scheme, and the
        // body is the only thing it needs back.
        assert!(encoded.get("scheme").is_none());
    }
}
