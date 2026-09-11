//! `ChangeRevision` — a sealed, immutable statement of what a Change did.
//!
//! Sealing binds four things together permanently: the exact definition in
//! force, the exact scope resolved once against a named base, the exact base
//! Baseline, and the exact project state that resulted.
//!
//! # Why sealing verifies the scope again
//!
//! The scope was resolved once, before the work. Sealing checks that what the
//! revision actually reached is inside it. Without that check, resolving once
//! would only constrain what the author *intended* to touch — and a change that
//! wandered outside its reviewed scope would seal happily, be reviewed against
//! a scope it no longer matched, and promote.
//!
//! So resolution and verification are two halves of one guarantee: resolution
//! fixes the boundary before the work, sealing proves the work stayed inside
//! it.
//!
//! # Nothing carries across a revision
//!
//! Evidence and Assessments bind an exact `ChangeRevisionId`, and a new
//! revision is a new identity. That is deliberate: evidence produced about
//! earlier bytes says nothing about the current ones, and letting it carry
//! forward would mean a change could be re-worked after approval and still
//! present the old approval as current.

use std::collections::BTreeSet;

use draft_dcg_contract::ids::{ActorId, ChangeId, ChangeRevisionId, ResourceId};
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::{BaselineId, Digest, ProjectStateRoot};
use serde::{Deserialize, Serialize};

use crate::dcg::definition::{ChangeDefinition, ScopeResolution};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::hashing::try_canonical_hash;

/// A sealed revision of a Change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeRevision {
    pub id: ChangeRevisionId,
    pub change: ChangeId,
    /// The exact definition in force when this was sealed.
    pub definition: Digest,
    /// The exact scope resolution it was worked within.
    pub scope: Digest,
    /// The Baseline it was worked from.
    pub base_baseline: BaselineId,
    /// The material state this revision proposes.
    pub project_state_root: ProjectStateRoot,
    /// The resources this revision actually touched.
    pub touched: BTreeSet<ResourceId>,
    pub sealed_by: ActorId,
    pub sealed_at: Timestamp,
}

impl ChangeRevision {
    pub fn digest(&self) -> DraftResult<Digest> {
        Digest::parse(try_canonical_hash(self)?)
            .map_err(|error| DraftError::new(DraftErrorKind::CorruptData, error.to_string()))
    }

    /// Seal a revision, verifying it against the scope it was worked within.
    ///
    /// This is where "resolved once" becomes enforceable. A revision that
    /// touched anything outside its resolved scope is refused rather than
    /// sealed, because a reviewer approving the scope would otherwise be
    /// approving something else.
    #[allow(clippy::too_many_arguments)]
    pub fn seal(
        id: ChangeRevisionId,
        definition: &ChangeDefinition,
        scope: &ScopeResolution,
        project_state_root: ProjectStateRoot,
        touched: BTreeSet<ResourceId>,
        sealed_by: ActorId,
        sealed_at: Timestamp,
    ) -> DraftResult<Self> {
        scope.validate_against(definition)?;

        if let Some(outside) = touched.difference(&scope.resources).next() {
            return Err(DraftError::new(
                DraftErrorKind::ProtectedResourceAccess,
                format!(
                    "resource '{outside}' was touched but is outside the resolved scope; a \
                     reviewer approving this scope would be approving something else"
                ),
            )
            .with_suggestion(
                "Amend the definition and re-resolve the scope, so the reviewed boundary and the \
                 effective one stay the same.",
            ));
        }

        Ok(Self {
            id,
            change: definition.change.clone(),
            definition: definition.digest()?,
            scope: scope.digest()?,
            base_baseline: scope.base_baseline.clone(),
            project_state_root,
            touched,
            sealed_by,
            sealed_at,
        })
    }

    /// Check a sealed revision against the facts it names.
    pub fn verify_against(
        &self,
        definition: &ChangeDefinition,
        scope: &ScopeResolution,
    ) -> DraftResult<()> {
        if self.definition != definition.digest()? {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!("revision '{}' names a different definition", self.id),
            ));
        }
        if self.scope != scope.digest()? {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!("revision '{}' names a different scope resolution", self.id),
            ));
        }
        if self.base_baseline != scope.base_baseline {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "revision '{}' was worked from a different Baseline than its scope was \
                     resolved against",
                    self.id
                ),
            ));
        }
        Ok(())
    }
}

/// Create-once storage for sealed [`ChangeRevision`]s.
///
/// A sealed revision is stored under its `rev_` logical id rather than its
/// digest, because everything that binds to a revision — Evidence, Assessments,
/// Reviews, Decisions, Gate evaluations — carries that id and not the digest.
/// §2.45's binding is what makes those references *exact*: the id resolves to
/// one canonical payload for the life of the project, and every read proves it.
///
/// Without this, "binds an exact revision" would degrade into "binds the same
/// opaque `rev_` string", and a substituted revision would silently inherit
/// every judgement made about the original.
pub struct RevisionStore {
    revisions: crate::support::immutable_store::ImmutableFactStore<ChangeRevision>,
}

impl RevisionStore {
    pub fn new(directory: impl Into<std::path::PathBuf>) -> Self {
        Self {
            revisions: crate::support::immutable_store::ImmutableFactStore::new(directory),
        }
    }

    /// Seal a revision into storage.
    ///
    /// Re-sealing byte-identical content succeeds idempotently; the same id
    /// with different content is refused as an integrity violation rather than
    /// overwriting what reviewers already judged.
    pub fn put(&self, revision: &ChangeRevision) -> DraftResult<()> {
        self.revisions.put(revision.id.as_str(), revision)?;
        Ok(())
    }

    /// Every sealed revision this project holds.
    pub fn list(&self) -> DraftResult<Vec<ChangeRevision>> {
        let mut found = Vec::new();
        for id in self.revisions.list_ids()? {
            if let Some(revision) = self.revisions.get(&id)? {
                found.push(revision);
            }
        }
        Ok(found)
    }

    pub fn get(&self, id: &ChangeRevisionId) -> DraftResult<Option<ChangeRevision>> {
        self.revisions.get(id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(name: &str) -> ResourceId {
        ResourceId::parse(format!("res_{name}")).unwrap()
    }

    fn set(names: &[&str]) -> BTreeSet<ResourceId> {
        names.iter().map(|name| resource(name)).collect()
    }

    fn definition(scope: &[&str]) -> ChangeDefinition {
        ChangeDefinition {
            change: ChangeId::parse("chg_000000000001").unwrap(),
            intent: "update the catalogue".into(),
            scope_declaration: set(scope),
            created_by: ActorId::parse("act_000000000001").unwrap(),
            created_at: Timestamp::from_unix_nanos(1_000),
        }
    }

    fn resolution(definition: &ChangeDefinition, present: &[&str]) -> ScopeResolution {
        ScopeResolution::resolve(
            definition,
            BaselineId::new(Digest::of_bytes(b"base")),
            &set(present),
            Timestamp::from_unix_nanos(2_000),
        )
        .unwrap()
    }

    fn seal(touched: &[&str]) -> DraftResult<ChangeRevision> {
        let definition = definition(&["a", "b"]);
        let scope = resolution(&definition, &["a", "b"]);
        ChangeRevision::seal(
            ChangeRevisionId::parse("rev_000000000001").unwrap(),
            &definition,
            &scope,
            ProjectStateRoot::new(Digest::of_bytes(b"state")),
            set(touched),
            ActorId::parse("act_000000000001").unwrap(),
            Timestamp::from_unix_nanos(3_000),
        )
    }

    #[test]
    fn a_revision_inside_its_scope_seals() {
        let sealed = seal(&["a"]).unwrap();
        assert_eq!(sealed.touched, set(&["a"]));
        assert_eq!(sealed.change, definition(&["a", "b"]).change);
    }

    #[test]
    fn touching_nothing_is_a_valid_revision() {
        // A change that turned out to need no edits is a real outcome, not an
        // error.
        seal(&[]).unwrap();
    }

    #[test]
    fn a_revision_that_wandered_outside_its_scope_is_refused() {
        // Without this, resolving once would only constrain what the author
        // intended to touch, and a reviewer approving the scope would be
        // approving something else.
        let error = seal(&["a", "elsewhere"]).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::ProtectedResourceAccess);
        assert!(
            error.message.contains("outside the resolved scope"),
            "{}",
            error.message
        );
    }

    #[test]
    fn sealing_binds_the_definition_scope_and_base_together() {
        let definition = definition(&["a", "b"]);
        let scope = resolution(&definition, &["a", "b"]);
        let sealed = seal(&["a"]).unwrap();
        sealed.verify_against(&definition, &scope).unwrap();

        // An amended definition no longer matches the sealed revision.
        let amended = definition_with_intent("something else");
        assert!(sealed.verify_against(&amended, &scope).is_err());
    }

    fn definition_with_intent(intent: &str) -> ChangeDefinition {
        let mut definition = definition(&["a", "b"]);
        definition.intent = intent.into();
        definition
    }

    #[test]
    fn a_revision_cannot_claim_a_scope_resolved_against_another_base() {
        let definition = definition(&["a", "b"]);
        let scope = resolution(&definition, &["a", "b"]);
        let mut sealed = seal(&["a"]).unwrap();
        sealed.base_baseline = BaselineId::new(Digest::of_bytes(b"elsewhere"));

        let error = sealed.verify_against(&definition, &scope).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::CorruptData);
    }

    #[test]
    fn sealing_refuses_a_scope_that_does_not_resolve_its_definition() {
        // Scenario J again, at the seal boundary: an amended definition leaves
        // the old resolution unusable rather than silently applicable.
        let amended = definition_with_intent("amended");
        let stale_scope = resolution(&definition(&["a", "b"]), &["a", "b"]);
        assert!(ChangeRevision::seal(
            ChangeRevisionId::parse("rev_000000000001").unwrap(),
            &amended,
            &stale_scope,
            ProjectStateRoot::new(Digest::of_bytes(b"state")),
            set(&["a"]),
            ActorId::parse("act_000000000001").unwrap(),
            Timestamp::from_unix_nanos(3_000),
        )
        .is_err());
    }

    #[test]
    fn two_revisions_of_one_change_have_different_identities() {
        // Scenario B: nothing carries across. Evidence bound to one revision
        // says nothing about the other.
        let first = seal(&["a"]).unwrap();
        let mut second = seal(&["a", "b"]).unwrap();
        second.id = ChangeRevisionId::parse("rev_000000000002").unwrap();
        assert_ne!(first.id, second.id);
        assert_ne!(first.digest().unwrap(), second.digest().unwrap());
    }

    #[test]
    fn the_sealed_state_is_part_of_the_revisions_identity() {
        let definition = definition(&["a", "b"]);
        let scope = resolution(&definition, &["a", "b"]);
        let build = |root: &[u8]| {
            ChangeRevision::seal(
                ChangeRevisionId::parse("rev_000000000001").unwrap(),
                &definition,
                &scope,
                ProjectStateRoot::new(Digest::of_bytes(root)),
                set(&["a"]),
                ActorId::parse("act_000000000001").unwrap(),
                Timestamp::from_unix_nanos(3_000),
            )
            .unwrap()
        };
        assert_ne!(
            build(b"state-one").digest().unwrap(),
            build(b"state-two").digest().unwrap()
        );
    }
}

// ---- Revision state and evidence invalidation ----

// Canonical revision state and digest-dependent evidence invalidation.

use crate::support::common::{now, OperationId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionState {
    Draft,
    Verified,
    Reviewing,
    Approved,
    Rejected,
    Submitted,
}

impl RevisionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Verified => "verified",
            Self::Reviewing => "reviewing",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Submitted => "submitted",
        }
    }

    pub fn may_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Draft, Self::Verified)
                | (Self::Verified, Self::Reviewing)
                | (Self::Reviewing, Self::Approved)
                | (Self::Reviewing, Self::Rejected)
                | (Self::Approved, Self::Submitted)
        )
    }

    pub fn transition(self, next: Self) -> DraftResult<Self> {
        if self.may_transition_to(next) {
            Ok(next)
        } else {
            Err(DraftError::invalid_config(format!(
                "invalid revision state transition {self:?} -> {next:?}"
            )))
        }
    }

    pub fn is_content_mutable(self) -> bool {
        self == Self::Draft
    }

    pub fn valid_actions(self) -> &'static [&'static str] {
        match self {
            Self::Draft => &["verify"],
            Self::Verified => &["review", "reopen"],
            Self::Reviewing => &["approve", "reject", "reopen"],
            Self::Approved => &["submit", "reopen"],
            Self::Rejected => &["reopen"],
            // Rollback resolves an explicit receipt/checkpoint target and is
            // not a direct mutation of the immutable submitted Change.
            Self::Submitted => &[],
        }
    }
}

pub fn valid_actions_for_label(label: &str) -> &'static [&'static str] {
    match label {
        "draft" => RevisionState::Draft.valid_actions(),
        "verified" => RevisionState::Verified.valid_actions(),
        "reviewing" => RevisionState::Reviewing.valid_actions(),
        "approved" => RevisionState::Approved.valid_actions(),
        "rejected" => RevisionState::Rejected.valid_actions(),
        "submitted" => RevisionState::Submitted.valid_actions(),
        _ => &[],
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionStateRecord {
    pub schema_version: u32,
    pub change_id: String,
    pub revision_id: String,
    pub revision_digest: String,
    pub lifecycle: RevisionState,
    pub updated_at: crate::support::common::Timestamp,
    pub last_operation_id: OperationId,
}

impl crate::contracts::VersionedContract for RevisionStateRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::RevisionState;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionTransitionRequest {
    pub operation_id: OperationId,
    pub expected_revision_id: String,
    pub expected_revision_digest: String,
    pub target: RevisionState,
}

impl RevisionStateRecord {
    pub fn transition(&mut self, request: RevisionTransitionRequest) -> DraftResult<()> {
        if request.expected_revision_id != self.revision_id
            || request.expected_revision_digest != self.revision_digest
        {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "Change revision or digest changed before the state transition",
            ));
        }
        if !self.lifecycle.may_transition_to(request.target) {
            return Err(DraftError::invalid_config(format!(
                "invalid revision state transition {:?} -> {:?}",
                self.lifecycle, request.target
            )));
        }
        self.lifecycle = request.target;
        self.updated_at = now();
        self.last_operation_id = request.operation_id;
        Ok(())
    }

    /// Reopening never edits a verified/reviewed envelope in place. It creates
    /// the next content-mutable revision; submitted Changes require a successor.
    pub fn reopen(
        &self,
        operation_id: OperationId,
        new_revision_id: String,
        new_revision_digest: String,
    ) -> DraftResult<Self> {
        if self.lifecycle == RevisionState::Submitted {
            return Err(DraftError::invalid_config(
                "submitted Changes are immutable; create a successor Change",
            ));
        }
        if self.lifecycle == RevisionState::Draft {
            return Err(DraftError::invalid_config("draft change is already open"));
        }
        Ok(Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::RevisionState,
            ),
            change_id: self.change_id.clone(),
            revision_id: new_revision_id,
            revision_digest: new_revision_digest,
            lifecycle: RevisionState::Draft,
            updated_at: now(),
            last_operation_id: operation_id,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceDependency {
    pub kind: String,
    pub subject_digest: String,
    pub dependency_digests: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRecord {
    pub schema_version: u32,
    pub evidence_id: String,
    pub subject_pack_id: String,
    pub subject_revision: u64,
    pub subject_digest: String,
    pub dependencies: Vec<EvidenceDependency>,
    pub valid: bool,
    pub invalidated_at: Option<crate::support::common::Timestamp>,
    pub invalidation_reason: Option<String>,
    pub superseding_revision: Option<u64>,
}

impl crate::contracts::VersionedContract for EvidenceRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::LifecycleEvidence;
}

pub fn invalidate_for_revision(
    evidence: &mut [EvidenceRecord],
    new_revision: u64,
    new_subject_digest: &str,
    changed_dependency_digests: &[String],
) -> usize {
    let mut invalidated = 0;
    for record in evidence.iter_mut().filter(|record| record.valid) {
        let subject_changed = record.subject_digest != new_subject_digest;
        let dependency_changed = record.dependencies.iter().any(|dependency| {
            dependency.subject_digest != new_subject_digest
                || dependency
                    .dependency_digests
                    .iter()
                    .any(|digest| changed_dependency_digests.contains(digest))
        });
        if subject_changed || dependency_changed {
            record.valid = false;
            record.invalidated_at = Some(now());
            record.invalidation_reason = Some(if subject_changed {
                "subject_digest_changed".into()
            } else {
                "declared_dependency_changed".into()
            });
            record.superseding_revision = Some(new_revision);
            invalidated += 1;
        }
    }
    invalidated
}

#[cfg(test)]
mod revision_state_tests {
    use super::*;

    #[test]
    fn lifecycle_is_strict_and_reopen_creates_a_revision() {
        let mut record = RevisionStateRecord {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::LifecycleEvidence,
            ),
            change_id: "chg_a".into(),
            revision_id: "rev_a".into(),
            revision_digest: "sha256:a".into(),
            lifecycle: RevisionState::Draft,
            updated_at: now(),
            last_operation_id: OperationId::new("op_create"),
        };
        record
            .transition(RevisionTransitionRequest {
                operation_id: OperationId::new("op_verify"),
                expected_revision_id: "rev_a".into(),
                expected_revision_digest: "sha256:a".into(),
                target: RevisionState::Verified,
            })
            .unwrap();
        let reopened = record
            .reopen(
                OperationId::new("op_reopen"),
                "rev_b".into(),
                "sha256:b".into(),
            )
            .unwrap();
        assert_eq!(reopened.revision_id, "rev_b");
        assert_eq!(reopened.lifecycle, RevisionState::Draft);
    }

    #[test]
    fn all_digest_dependent_evidence_is_preserved_but_invalidated() {
        let mut evidence = vec![EvidenceRecord {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::LifecycleEvidence,
            ),
            evidence_id: "ev_a".into(),
            subject_pack_id: "chg_a".into(),
            subject_revision: 1,
            subject_digest: "sha256:old".into(),
            dependencies: vec![],
            valid: true,
            invalidated_at: None,
            invalidation_reason: None,
            superseding_revision: None,
        }];
        assert_eq!(
            invalidate_for_revision(&mut evidence, 2, "sha256:new", &[]),
            1
        );
        assert!(!evidence[0].valid);
        assert_eq!(evidence[0].superseding_revision, Some(2));
    }

    #[test]
    fn valid_actions_are_computed_by_the_canonical_lifecycle() {
        assert_eq!(RevisionState::Draft.valid_actions(), &["verify"]);
        assert_eq!(valid_actions_for_label("approved"), &["submit", "reopen"]);
        assert!(valid_actions_for_label("future-state").is_empty());
    }
}
