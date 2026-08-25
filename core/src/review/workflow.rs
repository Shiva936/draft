//! Evidence, decisions, waivers, inbox derivation, and shared submit readiness.

use crate::support::common::{now, EvidenceId, Timestamp};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil::{list_with_extension, write_json};
use crate::workspace::layout::DraftLayout;
use serde::{Deserialize, Serialize};
use std::path::Path;

crate::id_newtype!(DecisionId, "dec_");
crate::id_newtype!(ReviewCommentId, "rcom_");
crate::id_newtype!(WaiverId, "wvr_");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceState {
    Fresh,
    Stale,
    Missing,
    Failed,
    Waived,
    Running,
    NotApplicable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRecord {
    pub schema_version: u32,
    pub id: EvidenceId,
    pub task_id: Option<String>,
    pub pack_id: Option<String>,
    pub execution_id: Option<String>,
    pub kind: String,
    pub state: EvidenceState,
    pub result: serde_json::Value,
    pub produced_at: Timestamp,
    pub stale_reason: Option<String>,
    pub invalidated_by: Vec<String>,
    pub receipt_id: Option<String>,
}

impl crate::contracts::VersionedContract for EvidenceRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::WorkflowEvidence;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionType {
    Approve,
    Reject,
    Waive,
    Override,
    AcceptRisk,
    RequestChanges,
    Invalidate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRecord {
    pub schema_version: u32,
    pub id: DecisionId,
    pub decision_type: DecisionType,
    pub task_id: Option<String>,
    pub pack_id: Option<String>,
    pub execution_id: Option<String>,
    pub evidence_ids: Vec<String>,
    pub receipt_id: Option<String>,
    pub base_stable_head: String,
    pub author: String,
    pub reason: String,
    pub created_at: Timestamp,
    pub invalidated_at: Option<Timestamp>,
    pub invalidation_reason: Option<String>,
}

impl crate::contracts::VersionedContract for DecisionRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::DecisionRecord;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Waiver {
    pub schema_version: u32,
    pub id: WaiverId,
    pub pack_id: String,
    pub finding_id: String,
    pub author: String,
    pub reason: String,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub receipt_id: Option<String>,
}

impl crate::contracts::VersionedContract for Waiver {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::Waiver;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadinessCheck {
    pub id: String,
    pub passed: bool,
    pub reason: String,
    pub next_action: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitReadinessView {
    pub pack_id: String,
    pub ready: bool,
    pub checks: Vec<ReadinessCheck>,
    pub computed_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxItem {
    pub schema_version: u32,
    pub id: String,
    pub kind: String,
    pub subject_id: String,
    pub status: String,
    pub summary: String,
    pub next_action: String,
}

impl crate::contracts::VersionedContract for InboxItem {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::InboxItem;
}

pub struct WorkflowStore {
    paths: DraftLayout,
}
impl WorkflowStore {
    pub fn for_root(root: &Path) -> Self {
        Self {
            paths: DraftLayout::for_root(root),
        }
    }
    pub fn evidence(&self) -> DraftResult<Vec<EvidenceRecord>> {
        load_dir(&self.paths.evidence_dir())
    }
    pub fn decisions(&self) -> DraftResult<Vec<DecisionRecord>> {
        load_dir(&self.paths.decisions_dir())
    }
    pub fn waivers(&self) -> DraftResult<Vec<Waiver>> {
        load_dir(&self.paths.waivers_dir())
    }
    pub fn write_evidence(&self, r: &EvidenceRecord) -> DraftResult<()> {
        write_json(&self.paths.evidence_dir().join(format!("{}.json", r.id)), r)
    }
    pub fn write_decision(&self, r: &DecisionRecord) -> DraftResult<()> {
        if r.reason.trim().is_empty() {
            return Err(DraftError::invalid_config(
                "decision reason cannot be empty",
            ));
        }
        write_json(
            &self.paths.decisions_dir().join(format!("{}.json", r.id)),
            r,
        )
    }
    pub fn write_waiver(&self, r: &Waiver) -> DraftResult<()> {
        if r.reason.trim().is_empty() || r.expires_at <= r.created_at {
            return Err(DraftError::invalid_config(
                "waiver requires a reason and future expiry",
            ));
        }
        write_json(&self.paths.waivers_dir().join(format!("{}.json", r.id)), r)
    }
    pub fn mark_pack_evidence_stale(
        &self,
        pack: &str,
        reason: &str,
        event: &str,
    ) -> DraftResult<usize> {
        let mut count = 0;
        for mut r in self.evidence()? {
            if r.pack_id.as_deref() == Some(pack) && r.state == EvidenceState::Fresh {
                r.state = EvidenceState::Stale;
                r.stale_reason = Some(reason.into());
                r.invalidated_by.push(event.into());
                self.write_evidence(&r)?;
                count += 1;
            }
        }
        Ok(count)
    }
    pub fn invalidate_approvals(&self, pack: &str, reason: &str) -> DraftResult<usize> {
        let mut count = 0;
        for mut d in self.decisions()? {
            if d.pack_id.as_deref() == Some(pack)
                && d.decision_type == DecisionType::Approve
                && d.invalidated_at.is_none()
            {
                d.invalidated_at = Some(now());
                d.invalidation_reason = Some(reason.into());
                self.write_decision(&d)?;
                count += 1;
            }
        }
        Ok(count)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn readiness(
        &self,
        pack: &str,
        pack_exists: bool,
        base_valid: bool,
        no_conflicts: bool,
        no_protected: bool,
        no_forbidden: bool,
        hooks_ready: bool,
        rollback_available: bool,
    ) -> DraftResult<SubmitReadinessView> {
        let evidence = self
            .evidence()?
            .into_iter()
            .filter(|e| e.pack_id.as_deref() == Some(pack))
            .collect::<Vec<_>>();
        let decisions = self.decisions()?;
        let approved = decisions.iter().any(|d| {
            d.pack_id.as_deref() == Some(pack)
                && d.decision_type == DecisionType::Approve
                && d.invalidated_at.is_none()
        });
        let fresh = !evidence.is_empty()
            && evidence.iter().all(|e| {
                matches!(
                    e.state,
                    EvidenceState::Fresh | EvidenceState::Waived | EvidenceState::NotApplicable
                )
            });
        let valid_waivers = self
            .waivers()?
            .into_iter()
            .all(|w| w.pack_id != pack || (w.expires_at > now() && !w.reason.trim().is_empty()));
        let request_changes_clear = !decisions.iter().any(|d| {
            d.pack_id.as_deref() == Some(pack)
                && d.decision_type == DecisionType::RequestChanges
                && d.invalidated_at.is_none()
        });
        let mut checks = vec![
            check(
                "pack_exists",
                pack_exists,
                "pack must exist",
                "draft list".to_string(),
            ),
            check(
                "approval",
                approved,
                "a current approval decision is required",
                format!("draft approve {pack} --reason <reason>"),
            ),
            check(
                "decision_record",
                approved || request_changes_clear,
                "blocking review decisions must be resolved",
                format!("draft review {pack}"),
            ),
            check(
                "evidence",
                fresh,
                "required evidence must be fresh",
                format!("draft verify {pack}"),
            ),
            check(
                "required_evidence",
                fresh,
                "required evidence must pass or be waived",
                format!("draft verify {pack}"),
            ),
            check(
                "owner_review",
                approved,
                "owner review is required before submit",
                format!("draft approve {pack} --reason <reason>"),
            ),
            check(
                "stable_head",
                base_valid,
                "pack base must match stable_head",
                format!("draft task {pack} --diff-stable"),
            ),
            check(
                "conflicts",
                no_conflicts,
                "unresolved conflicts block submission",
                format!("draft pack inspect {pack}"),
            ),
            check(
                "protected_files",
                no_protected,
                "protected files cannot be submitted",
                format!("draft pack inspect {pack}"),
            ),
            check(
                "forbidden_files",
                no_forbidden,
                "forbidden paths cannot be submitted",
                format!("draft pack inspect {pack}"),
            ),
            check(
                "approval_valid",
                approved,
                "approval must still be valid",
                format!("draft approve {pack} --reason <reason>"),
            ),
            check(
                "waivers",
                valid_waivers,
                "waivers must be valid and unexpired",
                format!("draft waive {pack} <finding> --reason <reason> --expires 7d"),
            ),
            check(
                "hooks_ready",
                hooks_ready,
                "submit hooks must be runnable",
                "draft hook run submit".to_string(),
            ),
            check(
                "rollback_target",
                rollback_available,
                "rollback target must be available",
                "draft checkpoint <message>".to_string(),
            ),
            check(
                "scope",
                no_forbidden && no_protected,
                "pack must stay inside allowed scope",
                format!("draft task {pack} --evidence"),
            ),
        ];
        let ready = checks.iter().all(|c| c.passed);
        if ready {
            for c in &mut checks {
                c.next_action = None;
            }
        }
        Ok(SubmitReadinessView {
            pack_id: pack.into(),
            ready,
            checks,
            computed_at: now(),
        })
    }
    pub fn inbox(&self) -> DraftResult<Vec<InboxItem>> {
        let mut out = Vec::new();
        for e in self.evidence()? {
            if matches!(
                e.state,
                EvidenceState::Stale | EvidenceState::Missing | EvidenceState::Failed
            ) {
                let subject = e
                    .pack_id
                    .clone()
                    .or(e.task_id.clone())
                    .unwrap_or_else(|| e.id.to_string());
                out.push(InboxItem {
                    schema_version: crate::contracts::current_version(
                        crate::contracts::ContractId::WorkflowEvidence,
                    ),
                    id: format!("inbox:evidence:{}", e.id),
                    kind: "evidence".into(),
                    subject_id: subject.clone(),
                    status: format!("{:?}", e.state).to_lowercase(),
                    summary: format!("{} evidence is {:?}", e.kind, e.state),
                    next_action: format!("draft verify {subject}"),
                });
            }
        }
        for d in self.decisions()? {
            if d.decision_type == DecisionType::RequestChanges && d.invalidated_at.is_none() {
                if let Some(pack) = d.pack_id {
                    out.push(InboxItem {
                        schema_version: crate::contracts::current_version(
                            crate::contracts::ContractId::InboxItem,
                        ),
                        id: format!("inbox:decision:{}", d.id),
                        kind: "request_changes".into(),
                        subject_id: pack.clone(),
                        status: "needs_review".into(),
                        summary: d.reason,
                        next_action: format!("draft review -p {pack}"),
                    });
                }
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }
}

fn load_dir<T: serde::de::DeserializeOwned + crate::contracts::VersionedContract>(
    dir: &Path,
) -> DraftResult<Vec<T>> {
    let mut out = Vec::new();
    for p in list_with_extension(dir, "json")? {
        out.push(crate::contracts::read_persisted(&p)?);
    }
    Ok(out)
}
fn check(id: &str, passed: bool, reason: &str, next: String) -> ReadinessCheck {
    ReadinessCheck {
        id: id.into(),
        passed,
        reason: if passed {
            "passed".into()
        } else {
            reason.into()
        },
        next_action: if passed { None } else { Some(next) },
    }
}

pub fn new_decision(
    kind: DecisionType,
    pack: String,
    base: String,
    author: String,
    reason: String,
) -> DraftResult<DecisionRecord> {
    if reason.trim().is_empty() {
        return Err(DraftError::new(
            DraftErrorKind::InvalidConfig,
            "decision reason cannot be empty",
        ));
    }
    Ok(DecisionRecord {
        schema_version: crate::contracts::current_version(crate::contracts::ContractId::InboxItem),
        id: DecisionId::generate(),
        decision_type: kind,
        task_id: None,
        pack_id: Some(pack),
        execution_id: None,
        evidence_ids: vec![],
        receipt_id: None,
        base_stable_head: base,
        author,
        reason,
        created_at: now(),
        invalidated_at: None,
        invalidation_reason: None,
    })
}
