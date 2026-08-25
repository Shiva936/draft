//! Canonical pack lifecycle and digest-dependent evidence invalidation.

use crate::support::common::{now, OperationId, Timestamp};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackLifecycle {
    Draft,
    Verified,
    Reviewing,
    Approved,
    Rejected,
    Submitted,
}

impl PackLifecycle {
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
                "invalid pack lifecycle transition {self:?} -> {next:?}"
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
            // not a direct mutation of the immutable submitted pack.
            Self::Submitted => &[],
        }
    }
}

pub fn valid_actions_for_label(label: &str) -> &'static [&'static str] {
    match label {
        "draft" => PackLifecycle::Draft.valid_actions(),
        "verified" => PackLifecycle::Verified.valid_actions(),
        "reviewing" => PackLifecycle::Reviewing.valid_actions(),
        "approved" => PackLifecycle::Approved.valid_actions(),
        "rejected" => PackLifecycle::Rejected.valid_actions(),
        "submitted" => PackLifecycle::Submitted.valid_actions(),
        _ => &[],
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackLifecycleRecord {
    pub schema_version: u32,
    pub pack_id: String,
    pub revision_id: String,
    pub revision_digest: String,
    pub lifecycle: PackLifecycle,
    pub updated_at: Timestamp,
    pub last_operation_id: OperationId,
}

impl crate::contracts::VersionedContract for PackLifecycleRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::PackLifecycle;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackTransitionRequest {
    pub operation_id: OperationId,
    pub expected_revision_id: String,
    pub expected_revision_digest: String,
    pub target: PackLifecycle,
}

impl PackLifecycleRecord {
    pub fn transition(&mut self, request: PackTransitionRequest) -> DraftResult<()> {
        if request.expected_revision_id != self.revision_id
            || request.expected_revision_digest != self.revision_digest
        {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "pack revision or digest changed before lifecycle transition",
            ));
        }
        if !self.lifecycle.may_transition_to(request.target) {
            return Err(DraftError::invalid_config(format!(
                "invalid pack lifecycle transition {:?} -> {:?}",
                self.lifecycle, request.target
            )));
        }
        self.lifecycle = request.target;
        self.updated_at = now();
        self.last_operation_id = request.operation_id;
        Ok(())
    }

    /// Reopening never edits a verified/reviewed envelope in place. It creates
    /// the next content-mutable revision; submitted packs require a successor.
    pub fn reopen(
        &self,
        operation_id: OperationId,
        new_revision_id: String,
        new_revision_digest: String,
    ) -> DraftResult<Self> {
        if self.lifecycle == PackLifecycle::Submitted {
            return Err(DraftError::invalid_config(
                "submitted packs are immutable; create a successor pack",
            ));
        }
        if self.lifecycle == PackLifecycle::Draft {
            return Err(DraftError::invalid_config("draft pack is already open"));
        }
        Ok(Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::PackLifecycle,
            ),
            pack_id: self.pack_id.clone(),
            revision_id: new_revision_id,
            revision_digest: new_revision_digest,
            lifecycle: PackLifecycle::Draft,
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
    pub invalidated_at: Option<Timestamp>,
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
mod tests {
    use super::*;

    #[test]
    fn lifecycle_is_strict_and_reopen_creates_a_revision() {
        let mut pack = PackLifecycleRecord {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::LifecycleEvidence,
            ),
            pack_id: "pck_a".into(),
            revision_id: "rev_a".into(),
            revision_digest: "sha256:a".into(),
            lifecycle: PackLifecycle::Draft,
            updated_at: now(),
            last_operation_id: OperationId::new("op_create"),
        };
        pack.transition(PackTransitionRequest {
            operation_id: OperationId::new("op_verify"),
            expected_revision_id: "rev_a".into(),
            expected_revision_digest: "sha256:a".into(),
            target: PackLifecycle::Verified,
        })
        .unwrap();
        let reopened = pack
            .reopen(
                OperationId::new("op_reopen"),
                "rev_b".into(),
                "sha256:b".into(),
            )
            .unwrap();
        assert_eq!(reopened.revision_id, "rev_b");
        assert_eq!(reopened.lifecycle, PackLifecycle::Draft);
    }

    #[test]
    fn all_digest_dependent_evidence_is_preserved_but_invalidated() {
        let mut evidence = vec![EvidenceRecord {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::LifecycleEvidence,
            ),
            evidence_id: "ev_a".into(),
            subject_pack_id: "pck_a".into(),
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
        assert_eq!(PackLifecycle::Draft.valid_actions(), &["verify"]);
        assert_eq!(valid_actions_for_label("approved"), &["submit", "reopen"]);
        assert!(valid_actions_for_label("future-state").is_empty());
    }
}
