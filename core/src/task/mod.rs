//! Canonical task contracts and crash-safe project-local task storage.

pub mod candidate;

use crate::support::common::{now, ExecutionId, TaskId, Timestamp};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil::{list_with_extension, write_json};
use crate::workspace::layout::DraftLayout;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskViewStatus {
    Blocked,
    Running,
    Approved,
    NeedsReview,
    Defined,
    Pending,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionView {
    pub execution_id: String,
    pub candidate: String,
    pub status: String,
    pub produced_pack: Option<String>,
    pub error: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskView {
    pub task: TaskDefinition,
    pub health: TaskViewStatus,
    pub latest_execution: Option<ExecutionView>,
    pub review_status: TaskViewStatus,
    pub recommended_action: String,
    pub execution_count: usize,
    pub evidence_count: usize,
    pub produced_packs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Defined,
    Inline,
    Imported,
    Generated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskMode {
    Normal,
    Safe,
    PlanFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskRisk {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskLifecycleStatus {
    #[default]
    Open,
    InProgress,
    Blocked,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    Low,
    #[default]
    Normal,
    High,
    Urgent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssigneeRef {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NextAction {
    pub id: String,
    pub label: String,
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSourceContext {
    pub path: String,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub symbol: Option<String>,
    pub reason: Option<String>,
}

/// When a task should run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSchedule {
    pub cron: Option<String>,
    pub note: Option<String>,
}

/// A question a human reviewer must be able to answer before approving.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewQuestion {
    pub question: String,
    pub blocking: bool,
}

impl ReviewQuestion {
    pub fn new(question: impl Into<String>) -> Self {
        ReviewQuestion {
            question: question.into(),
            blocking: false,
        }
    }
}

/// A deterministic template rule for splitting an oversized task into child
/// tasks (`draft task <task> --decompose`). No AI involved: each rule maps a
/// zone pattern to a child task shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecompositionRule {
    pub id: String,
    pub description: String,
    /// Zones the child task should own; the child forbids everything else the
    /// parent allowed.
    pub zones: Vec<String>,
    /// Template applied to the generated child task.
    pub child_template: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDefinition {
    pub schema_version: u32,
    pub id: TaskId,
    pub name: String,
    pub kind: TaskKind,
    pub template: Option<String>,
    pub goal: String,
    pub allowed_zones: Vec<String>,
    pub forbidden_zones: Vec<String>,
    pub success_criteria: Vec<String>,
    pub risk: TaskRisk,
    pub mode: TaskMode,
    pub required_evidence: Vec<String>,
    pub review_questions: Vec<ReviewQuestion>,
    pub candidate_preset: Option<String>,
    pub schedule: Option<TaskSchedule>,
    pub parent_pack: Option<String>,
    pub source_context: Option<TaskSourceContext>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub created_by: String,
    pub base_stable_head: String,
    pub status: TaskLifecycleStatus,
    pub priority: TaskPriority,
    pub due_at: Option<Timestamp>,
    pub next_actions: Vec<NextAction>,
    pub assignee_ref: Option<AssigneeRef>,
    pub metadata: BTreeMap<String, serde_json::Value>,
}

impl crate::contracts::VersionedContract for TaskDefinition {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::TaskDefinition;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTemplate {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub default_risk: TaskRisk,
    pub default_mode: TaskMode,
    pub default_required_evidence: Vec<String>,
    pub default_review_questions: Vec<ReviewQuestion>,
    pub default_forbidden_zones: Vec<String>,
    pub recommended_candidate_preset: Option<String>,
    pub success_criteria_shape: Vec<String>,
    pub decomposition_rules: Vec<DecompositionRule>,
}

impl crate::contracts::VersionedContract for TaskTemplate {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::TaskTemplate;
}

/// The ten built-in template ids, in presentation order.
pub const BUILTIN_TEMPLATE_IDS: [&str; 10] = [
    "bug_fix",
    "small_feature",
    "refactor",
    "test_gap",
    "security_sensitive",
    "dependency_audit",
    "performance_investigation",
    "docs_update",
    "ui_text_change",
    "migration_plan",
];

pub fn builtin_template(id: &str) -> DraftResult<TaskTemplate> {
    struct Spec {
        name: &'static str,
        risk: TaskRisk,
        mode: TaskMode,
        evidence: &'static [&'static str],
        questions: &'static [&'static str],
        forbidden: &'static [&'static str],
        criteria: &'static [&'static str],
        preset: &'static str,
        rules: &'static [(
            &'static str,
            &'static str,
            &'static [&'static str],
            &'static str,
        )],
    }
    let spec = match id {
        "bug_fix" => Spec {
            name: "Bug fix",
            risk: TaskRisk::Medium,
            mode: TaskMode::Normal,
            evidence: &["targeted_tests"],
            questions: &["Does the fix address the root cause?"],
            forbidden: &[],
            criteria: &["Reproduction no longer fails"],
            preset: "fast",
            rules: &[
                (
                    "reproduce",
                    "Cover the failure with a test first",
                    &["tests/**"],
                    "test_gap",
                ),
                (
                    "fix",
                    "Apply the smallest fix for the covered failure",
                    &["**"],
                    "bug_fix",
                ),
            ],
        },
        "small_feature" => Spec {
            name: "Small feature",
            risk: TaskRisk::Medium,
            mode: TaskMode::Normal,
            evidence: &["tests"],
            questions: &["Are acceptance criteria satisfied?"],
            forbidden: &[],
            criteria: &["Feature behavior is covered"],
            preset: "fast",
            rules: &[
                (
                    "core",
                    "Implement the feature behavior",
                    &["**"],
                    "small_feature",
                ),
                (
                    "tests",
                    "Cover the feature with tests",
                    &["tests/**"],
                    "test_gap",
                ),
                (
                    "docs",
                    "Document the feature",
                    &["docs/**", "README.md"],
                    "docs_update",
                ),
            ],
        },
        "refactor" => Spec {
            name: "Refactor",
            risk: TaskRisk::Medium,
            mode: TaskMode::PlanFirst,
            evidence: &["full_tests"],
            questions: &["Is behavior preserved?"],
            forbidden: &[],
            criteria: &["Public behavior is unchanged"],
            preset: "strict",
            rules: &[
                (
                    "tests-first",
                    "Lock behavior in with tests before moving code",
                    &["tests/**"],
                    "test_gap",
                ),
                (
                    "move",
                    "Perform the mechanical refactor",
                    &["**"],
                    "refactor",
                ),
            ],
        },
        "test_gap" => Spec {
            name: "Test gap",
            risk: TaskRisk::Low,
            mode: TaskMode::Normal,
            evidence: &["tests"],
            questions: &["Does the test fail without the fix?"],
            forbidden: &[],
            criteria: &["Gap is reproducibly covered"],
            preset: "fast",
            rules: &[],
        },
        "security_sensitive" => Spec {
            name: "Security-sensitive change",
            risk: TaskRisk::Critical,
            mode: TaskMode::Safe,
            evidence: &["full_tests", "security_review"],
            questions: &["Can secrets or privileges cross this boundary?"],
            forbidden: &[".env", "*.key", "*.pem", "secrets/**"],
            criteria: &["Threat and regression cases pass"],
            preset: "paranoid",
            rules: &[
                (
                    "boundary",
                    "Isolate the security boundary change",
                    &["**"],
                    "security_sensitive",
                ),
                (
                    "tests",
                    "Add threat/regression coverage",
                    &["tests/**"],
                    "test_gap",
                ),
            ],
        },
        "dependency_audit" => Spec {
            name: "Dependency audit",
            risk: TaskRisk::High,
            mode: TaskMode::PlanFirst,
            evidence: &["dependency_audit", "full_tests"],
            questions: &["Are provenance and licenses acceptable?"],
            forbidden: &[],
            criteria: &["Dependency delta is justified"],
            preset: "strict",
            rules: &[],
        },
        "performance_investigation" => Spec {
            name: "Performance investigation",
            risk: TaskRisk::Medium,
            mode: TaskMode::PlanFirst,
            evidence: &["benchmark"],
            questions: &["Is the comparison reproducible?"],
            forbidden: &[],
            criteria: &["Baseline and result are recorded"],
            preset: "strict",
            rules: &[
                (
                    "baseline",
                    "Record the reproducible baseline",
                    &["benches/**", "tests/**"],
                    "test_gap",
                ),
                (
                    "change",
                    "Apply and measure the candidate change",
                    &["**"],
                    "performance_investigation",
                ),
            ],
        },
        "docs_update" => Spec {
            name: "Docs update",
            risk: TaskRisk::Low,
            mode: TaskMode::Normal,
            evidence: &["docs_check"],
            questions: &["Does documentation match current behavior?"],
            forbidden: &[],
            criteria: &["Links and examples validate"],
            preset: "fast",
            rules: &[],
        },
        "ui_text_change" => Spec {
            name: "UI text change",
            risk: TaskRisk::Low,
            mode: TaskMode::Normal,
            evidence: &["ui_test"],
            questions: &["Is the text accessible and consistent?"],
            forbidden: &[],
            criteria: &["Affected states render correctly"],
            preset: "fast",
            rules: &[],
        },
        "migration_plan" => Spec {
            name: "Migration plan",
            risk: TaskRisk::High,
            mode: TaskMode::PlanFirst,
            evidence: &["migration_test", "rollback_test"],
            questions: &["Is rollback lossless?"],
            forbidden: &[],
            criteria: &["Forward and rollback paths pass"],
            preset: "paranoid",
            rules: &[
                (
                    "forward",
                    "Implement the forward migration",
                    &["migrations/**"],
                    "migration_plan",
                ),
                (
                    "rollback",
                    "Implement and test the rollback path",
                    &["migrations/**", "tests/**"],
                    "migration_plan",
                ),
            ],
        },
        _ => {
            return Err(DraftError::not_found(format!(
                "unknown task template '{id}'"
            )))
        }
    };
    Ok(TaskTemplate {
        schema_version: crate::contracts::current_version(
            crate::contracts::ContractId::TaskDefinition,
        ),
        id: id.into(),
        name: spec.name.into(),
        default_risk: spec.risk,
        default_mode: spec.mode,
        default_required_evidence: spec.evidence.iter().map(|s| s.to_string()).collect(),
        default_review_questions: spec
            .questions
            .iter()
            .map(|q| ReviewQuestion::new(*q))
            .collect(),
        default_forbidden_zones: spec.forbidden.iter().map(|s| s.to_string()).collect(),
        recommended_candidate_preset: Some(spec.preset.into()),
        success_criteria_shape: spec.criteria.iter().map(|s| s.to_string()).collect(),
        decomposition_rules: spec
            .rules
            .iter()
            .map(|(id, description, zones, child)| DecompositionRule {
                id: id.to_string(),
                description: description.to_string(),
                zones: zones.iter().map(|z| z.to_string()).collect(),
                child_template: Some(child.to_string()),
            })
            .collect(),
    })
}

pub fn builtin_templates() -> Vec<TaskTemplate> {
    BUILTIN_TEMPLATE_IDS
        .iter()
        .map(|id| builtin_template(id).expect("built-in template ids resolve"))
        .collect()
}

pub fn apply_template(task: &mut TaskDefinition, id: &str) -> DraftResult<()> {
    let t = builtin_template(id)?;
    task.template = Some(id.into());
    task.risk = t.default_risk;
    task.mode = t.default_mode;
    task.required_evidence = t.default_required_evidence;
    task.review_questions = t.default_review_questions;
    task.success_criteria = t.success_criteria_shape;
    if task.candidate_preset.is_none() {
        task.candidate_preset = t.recommended_candidate_preset;
    }
    for zone in t.default_forbidden_zones {
        if !task.forbidden_zones.contains(&zone) {
            task.forbidden_zones.push(zone);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Queued,
    Running,
    Cancelled,
    Interrupted,
    Retrying,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Execution {
    pub schema_version: u32,
    pub id: ExecutionId,
    pub task_id: TaskId,
    pub candidate: String,
    pub status: ExecutionStatus,
    pub attempt: u32,
    pub previous_attempt: Option<ExecutionId>,
    pub command: Vec<String>,
    pub base_stable_head: String,
    pub parent_pack: Option<String>,
    pub produced_pack: Option<String>,
    pub evidence_ids: Vec<String>,
    pub receipt_ids: Vec<String>,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
    pub cancellation_reason: Option<String>,
    pub failure_reason: Option<String>,
    pub workspace_id: Option<String>,
    /// OS process id while the candidate command is running (used by cancel
    /// and interrupted-execution recovery).
    pub pid: Option<u32>,
    /// Object-store refs of captured candidate output.
    pub stdout_ref: Option<String>,
    pub stderr_ref: Option<String>,
    pub exit_code: Option<i32>,
    pub verification_result: Option<serde_json::Value>,
    pub scope_result: Option<serde_json::Value>,
    pub risk_result: Option<serde_json::Value>,
}

impl crate::contracts::VersionedContract for Execution {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::Execution;
}

impl Execution {
    pub fn queued(task: &TaskDefinition, candidate: String, command: Vec<String>) -> Self {
        Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::TaskTemplate,
            ),
            id: ExecutionId::generate(),
            task_id: task.id.clone(),
            candidate,
            status: ExecutionStatus::Queued,
            attempt: 1,
            previous_attempt: None,
            command,
            base_stable_head: task.base_stable_head.clone(),
            parent_pack: task.parent_pack.clone(),
            produced_pack: None,
            evidence_ids: vec![],
            receipt_ids: vec![],
            started_at: None,
            finished_at: None,
            cancellation_reason: None,
            failure_reason: None,
            workspace_id: None,
            pid: None,
            stdout_ref: None,
            stderr_ref: None,
            exit_code: None,
            verification_result: None,
            scope_result: None,
            risk_result: None,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            ExecutionStatus::Cancelled
                | ExecutionStatus::Interrupted
                | ExecutionStatus::Completed
                | ExecutionStatus::Failed
        )
    }

    pub fn is_resumable(&self) -> bool {
        matches!(
            self.status,
            ExecutionStatus::Interrupted | ExecutionStatus::Cancelled
        )
    }
}

pub struct ExecutionStore {
    paths: DraftLayout,
}
impl ExecutionStore {
    pub fn for_root(root: &Path) -> Self {
        Self {
            paths: DraftLayout::for_root(root),
        }
    }
    pub fn write(&self, e: &Execution) -> DraftResult<()> {
        self.paths.create_all()?;
        write_json(&self.paths.execution_file(e.id.as_str()), e)
    }
    pub fn read(&self, id: &str) -> DraftResult<Execution> {
        crate::contracts::read_persisted(&self.paths.execution_file(id))
    }
    pub fn list_for_task(&self, task: &TaskId) -> DraftResult<Vec<Execution>> {
        let mut out = Vec::new();
        for path in list_with_extension(&self.paths.executions_dir(), "json")? {
            let e: Execution = crate::contracts::read_persisted(&path)?;
            if &e.task_id == task {
                out.push(e);
            }
        }
        out.sort_by(|a, b| a.attempt.cmp(&b.attempt).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }
    pub fn list_all(&self) -> DraftResult<Vec<Execution>> {
        let mut out = Vec::new();
        for path in list_with_extension(&self.paths.executions_dir(), "json")? {
            out.push(crate::contracts::read_persisted::<Execution>(&path)?);
        }
        out.sort_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    /// Apply a mutation and persist it atomically.
    pub fn update<F: FnOnce(&mut Execution)>(&self, id: &str, f: F) -> DraftResult<Execution> {
        let mut e = self.read(id)?;
        f(&mut e);
        self.write(&e)?;
        Ok(e)
    }

    pub fn mark_running(&self, id: &str, pid: Option<u32>) -> DraftResult<Execution> {
        self.update(id, |e| {
            e.status = ExecutionStatus::Running;
            e.pid = pid;
            e.started_at = Some(now());
        })
    }

    pub fn mark_completed(&self, id: &str) -> DraftResult<Execution> {
        self.update(id, |e| {
            e.status = ExecutionStatus::Completed;
            e.pid = None;
            e.finished_at = Some(now());
        })
    }

    pub fn mark_failed(&self, id: &str, reason: &str) -> DraftResult<Execution> {
        self.update(id, |e| {
            e.status = ExecutionStatus::Failed;
            e.pid = None;
            e.failure_reason = Some(reason.to_string());
            e.finished_at = Some(now());
        })
    }

    pub fn mark_cancelled(&self, id: &str, reason: &str) -> DraftResult<Execution> {
        let e = self.read(id)?;
        if e.is_terminal() {
            return Err(DraftError::invalid_config(format!(
                "execution {id} already finished ({:?}); nothing to cancel",
                e.status
            )));
        }
        self.update(id, |e| {
            e.status = ExecutionStatus::Cancelled;
            e.pid = None;
            e.cancellation_reason = Some(reason.to_string());
            e.finished_at = Some(now());
        })
    }

    pub fn mark_interrupted(&self, id: &str, reason: &str) -> DraftResult<Execution> {
        self.update(id, |e| {
            e.status = ExecutionStatus::Interrupted;
            e.pid = None;
            e.failure_reason = Some(reason.to_string());
            e.finished_at = Some(now());
        })
    }

    pub fn retry(&self, id: &str) -> DraftResult<Execution> {
        let old = self.read(id)?;
        if !matches!(
            old.status,
            ExecutionStatus::Failed | ExecutionStatus::Cancelled | ExecutionStatus::Interrupted
        ) {
            return Err(DraftError::invalid_config(
                "only failed, cancelled, or interrupted executions can be retried",
            ));
        }
        let mut next = old.clone();
        next.id = ExecutionId::generate();
        next.status = ExecutionStatus::Retrying;
        next.attempt += 1;
        next.previous_attempt = Some(old.id);
        next.started_at = None;
        next.finished_at = None;
        next.failure_reason = None;
        next.cancellation_reason = None;
        next.pid = None;
        next.stdout_ref = None;
        next.stderr_ref = None;
        next.exit_code = None;
        next.produced_pack = None;
        self.write(&next)?;
        Ok(next)
    }

    /// Delete every execution record (and runtime dir) belonging to a task.
    /// Returns the removed execution ids.
    pub fn drop_for_task(&self, task: &TaskId) -> DraftResult<Vec<String>> {
        let mut removed = Vec::new();
        for e in self.list_for_task(task)? {
            let file = self.paths.execution_file(e.id.as_str());
            if file.exists() {
                std::fs::remove_file(&file).map_err(|err| {
                    DraftError::storage(format!("failed to remove {}: {err}", file.display()))
                })?;
            }
            let runtime = self.paths.execution_runtime_dir(e.id.as_str());
            if runtime.exists() {
                let _ = std::fs::remove_dir_all(&runtime);
            }
            removed.push(e.id.to_string());
        }
        Ok(removed)
    }
}

impl TaskDefinition {
    pub fn new(
        name: String,
        goal: String,
        base_stable_head: String,
        created_by: String,
    ) -> DraftResult<Self> {
        validate_name(&name)?;
        if goal.trim().is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::InvalidConfig,
                "task goal cannot be empty",
            ));
        }
        let at = now();
        Ok(Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::Execution,
            ),
            id: TaskId::generate(),
            name,
            kind: TaskKind::Defined,
            template: None,
            goal,
            allowed_zones: vec!["**".into()],
            forbidden_zones: vec![".draft/**".into()],
            success_criteria: Vec::new(),
            risk: TaskRisk::Medium,
            mode: TaskMode::Normal,
            required_evidence: Vec::new(),
            review_questions: Vec::new(),
            candidate_preset: None,
            schedule: None,
            parent_pack: None,
            source_context: None,
            created_at: at,
            updated_at: at,
            created_by,
            base_stable_head,
            status: TaskLifecycleStatus::Open,
            priority: TaskPriority::Normal,
            due_at: None,
            next_actions: Vec::new(),
            assignee_ref: None,
            metadata: BTreeMap::new(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TaskIndex {
    schema_version: u32,
    by_name: BTreeMap<String, String>,
}

impl crate::contracts::VersionedContract for TaskIndex {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::TaskIndex;
}

pub struct TaskStore {
    paths: DraftLayout,
}

impl TaskStore {
    pub fn for_root(root: &Path) -> Self {
        Self {
            paths: DraftLayout::for_root(root),
        }
    }

    pub fn create(&self, task: &TaskDefinition) -> DraftResult<()> {
        self.paths.create_all()?;
        if self.resolve(&task.name)?.is_some() {
            return Err(DraftError::new(
                DraftErrorKind::TaskDefinitionConflict,
                format!("task '{}' already exists", task.name),
            )
            .with_suggestion(format!("run `draft task {}`", task.name)));
        }
        write_json(&self.paths.task_file(task.id.as_str()), task)?;
        self.rebuild_index()
    }

    pub fn import(&self, task: &TaskDefinition) -> DraftResult<()> {
        self.create(task)
    }

    pub fn export_to(&self, id_or_name: &str, output: &Path) -> DraftResult<TaskDefinition> {
        let task = self
            .resolve(id_or_name)?
            .ok_or_else(|| DraftError::not_found(format!("task '{id_or_name}' was not found")))?;
        write_json(output, &task)?;
        Ok(task)
    }

    pub fn resolve(&self, id_or_name: &str) -> DraftResult<Option<TaskDefinition>> {
        let direct = self.paths.task_file(id_or_name);
        if direct.exists() {
            return crate::contracts::read_persisted(&direct).map(Some);
        }
        Ok(self.list()?.into_iter().find(|t| t.name == id_or_name))
    }

    pub fn list(&self) -> DraftResult<Vec<TaskDefinition>> {
        let mut tasks: Vec<TaskDefinition> = Vec::new();
        for path in list_with_extension(&self.paths.tasks_dir(), "json")? {
            tasks.push(crate::contracts::read_persisted(&path)?);
        }
        tasks.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(tasks)
    }

    /// Persist changes to an existing task definition.
    pub fn update(&self, task: &TaskDefinition) -> DraftResult<()> {
        let file = self.paths.task_file(task.id.as_str());
        if !file.exists() {
            return Err(DraftError::not_found(format!(
                "task '{}' was not found",
                task.id
            )));
        }
        write_json(&file, task)?;
        self.rebuild_index()
    }

    /// Normal drop: clear executions/runtime state, keep the definition.
    /// Hard drop: delete definition and runtime state. Never touches
    /// repository source files.
    pub fn drop_task(&self, id_or_name: &str, hard: bool) -> DraftResult<TaskDropOutcome> {
        let task = self
            .resolve(id_or_name)?
            .ok_or_else(|| DraftError::not_found(format!("task '{id_or_name}' was not found")))?;
        let removed_executions =
            ExecutionStore::for_root(&self.paths.root()).drop_for_task(&task.id)?;
        let mut definition_removed = false;
        if hard {
            let file = self.paths.task_file(task.id.as_str());
            if file.exists() {
                std::fs::remove_file(&file).map_err(|err| {
                    DraftError::storage(format!("failed to remove {}: {err}", file.display()))
                })?;
            }
            definition_removed = true;
        }
        self.rebuild_index()?;
        Ok(TaskDropOutcome {
            task_id: task.id.to_string(),
            task_name: task.name,
            removed_executions,
            definition_removed,
        })
    }

    pub fn rebuild_index(&self) -> DraftResult<()> {
        let mut index = TaskIndex {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::TaskIndex,
            ),
            ..Default::default()
        };
        for task in self.list()? {
            index.by_name.insert(task.name, task.id.to_string());
        }
        write_json(&self.paths.task_name_index(), &index)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDropOutcome {
    pub task_id: String,
    pub task_name: String,
    pub removed_executions: Vec<String>,
    pub definition_removed: bool,
}

fn validate_name(name: &str) -> DraftResult<()> {
    let valid = !name.is_empty()
        && name.len() <= 80
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid {
        Ok(())
    } else {
        Err(DraftError::new(
            DraftErrorKind::InvalidConfig,
            "task name must be 1-80 ASCII letters, digits, '-' or '_'",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn create_resolve_and_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let task = TaskDefinition::new(
            "bug-1".into(),
            "Fix it".into(),
            "head".into(),
            "human".into(),
        )
        .unwrap();
        let store = TaskStore::for_root(dir.path());
        store.create(&task).unwrap();
        assert_eq!(store.resolve("bug-1").unwrap().unwrap().id, task.id);
        assert_eq!(
            store.create(&task).unwrap_err().kind,
            DraftErrorKind::TaskDefinitionConflict
        );
    }
}
