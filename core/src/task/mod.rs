//! Canonical task contracts and crash-safe project-local task storage.

pub mod candidate;

use crate::project::layout::DraftLayout;
use crate::support::common::{now, ExecutionId, TaskId, Timestamp};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil::{list_with_extension, write_json};
use crate::support::telemetry::Counter;
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
    pub produced_change: Option<String>,
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
    pub produced_changes: Vec<String>,
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

impl TaskLifecycleStatus {
    /// Whether the task is finished, either way it finished.
    ///
    /// `Completed` and `Cancelled` are different outcomes but the same
    /// lifecycle answer: no more work is planned. Everything else means the
    /// task is still open, including `Blocked` — blocked work is stalled, not
    /// finished, and treating it as closed would hide it from exactly the
    /// people who need to unblock it.
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
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
    /// The resource this task is about. Opaque: Draft never parses the body.
    pub locator: crate::dcg::resource::ResourceLocator,
    /// The contributed space `start` and `length` are expressed in — lines of a
    /// document, keys of a record set, frames of a timeline. Core stores it and
    /// compares it for equality; it never learns what a coordinate means.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinate_space: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
    /// A contributed element this task concerns, when extraction found one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    /// Monotonic, incremented by every accepted mutation.
    ///
    /// What makes a task update a compare-exchange rather than a
    /// last-writer-wins overwrite: two callers that both read generation N can
    /// no longer both succeed, so a concurrent edit is a detected conflict
    /// instead of a silently discarded one.
    #[serde(default)]
    pub generation: u64,
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
    pub parent_change: Option<String>,
    pub source_context: Option<TaskSourceContext>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub created_by: String,
    pub base_baseline: String,
    pub status: TaskLifecycleStatus,
    pub priority: TaskPriority,
    pub due_at: Option<Timestamp>,
    pub next_actions: Vec<NextAction>,
    pub assignee_ref: Option<AssigneeRef>,
    pub metadata: BTreeMap<String, serde_json::Value>,
}

impl crate::support::record_guard::RevisionedRecord for TaskDefinition {
    fn generation(&self) -> u64 {
        self.generation
    }
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

/// Resolve a contributed task template into the shape Core works with.
///
/// Draft ships no templates. "Fix a defect", "add tests", "migrate data" are
/// software-project vocabulary; a recording session's templates would be
/// entirely different ones. What Core owns is the *shape* — risk, mode,
/// evidence, questions, criteria, decomposition — which is domain-neutral, and
/// the guarantee that a template can only narrow a task, never widen it.
pub fn resolve_template(
    contributed: &draft_extension_contract::TaskTemplateContribution,
) -> DraftResult<TaskTemplate> {
    let mut decomposition_rules = Vec::new();
    for step in &contributed.steps {
        // A step's scope becomes the child's zones. Zones are path globs, so a
        // scope Core cannot express as one is refused rather than dropped:
        // silently unscoping a child would widen what an agent may touch, which
        // is the opposite of what a template is for.
        let zones = match &step.scope {
            None => Vec::new(),
            Some(draft_extension_contract::ResourcePredicate::Raw {
                of: draft_extension_contract::RawResourcePredicate::PathGlob { glob },
            }) => vec![glob.clone()],
            Some(_) => {
                return Err(DraftError::invalid_config(format!(
                    "task template '{}' scopes step '{}' with a predicate that is not a path \
                     glob, which a task zone cannot express",
                    contributed.template_id, step.id
                ))
                .with_suggestion(
                    "scope template steps with `path_glob`, or leave the step unscoped",
                ))
            }
        };
        decomposition_rules.push(DecompositionRule {
            id: step.id.clone(),
            description: step.label.clone(),
            zones,
            child_template: None,
        });
    }
    Ok(TaskTemplate {
        schema_version: crate::contracts::current_version(
            crate::contracts::ContractId::TaskDefinition,
        ),
        id: contributed.template_id.qualified(),
        name: contributed.display_name.clone(),
        // Neutral defaults. A template says what work looks like; how risky it
        // is, is what the risk rules decide from the change itself.
        default_risk: TaskRisk::Medium,
        default_mode: TaskMode::Normal,
        default_required_evidence: contributed
            .required_evidence
            .iter()
            .map(|id| id.qualified())
            .collect(),
        default_review_questions: contributed
            .review_questions
            .iter()
            .map(|question| ReviewQuestion::new(question.as_str()))
            .collect(),
        // Protections and view rules decide what is off-limits, project-wide.
        // A template narrows what a task is *about*, and does not carry its own
        // parallel prohibition list.
        default_forbidden_zones: Vec::new(),
        recommended_candidate_preset: None,
        success_criteria_shape: contributed.success_criteria.clone(),
        decomposition_rules,
    })
}

/// Apply a resolved template to a task.
pub fn apply_template(task: &mut TaskDefinition, template: &TaskTemplate) -> DraftResult<()> {
    let t = template.clone();
    task.template = Some(t.id.clone());
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
    pub base_baseline: String,
    pub parent_change: Option<String>,
    pub produced_change: Option<String>,
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
            base_baseline: task.base_baseline.clone(),
            parent_change: task.parent_change.clone(),
            produced_change: None,
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
        next.produced_change = None;
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
        base_baseline: String,
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
            generation: 0,
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
            parent_change: None,
            source_context: None,
            created_at: at,
            updated_at: at,
            created_by,
            base_baseline,
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
    records: crate::support::record_guard::RevisionedRecordStore<TaskDefinition>,
}

impl TaskStore {
    pub fn for_root(root: &Path) -> Self {
        let paths = DraftLayout::for_root(root);
        Self {
            records: crate::support::record_guard::RevisionedRecordStore::new(paths.tasks_dir())
                .with_order(crate::support::lock_order::LockOrder::DomainRecordStore)
                .counting_conflicts_as(Counter::TaskRecordCasConflicts),
            paths,
        }
    }

    /// The record store, for callers committing through the audited path.
    pub fn records(&self) -> &crate::support::record_guard::RevisionedRecordStore<TaskDefinition> {
        &self.records
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
        // `Absent` is the expected state: a task that already exists must not
        // be silently overwritten by a second create racing the first.
        self.records.compare_exchange(
            task.id.as_str(),
            &crate::support::record_guard::ExpectedRecordState::Absent,
            task,
        )?;
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
    ///
    /// A compare-exchange against the generation the caller read, under the
    /// task's own lock. Two callers that both read generation N can no longer
    /// both succeed: the second is told its edit is stale rather than having it
    /// silently overwrite the first.
    pub fn update(&self, task: &TaskDefinition) -> DraftResult<()> {
        self.records.with_locked_record(
            task.id.as_str(),
            crate::support::record_guard::DEFAULT_LOCK_TIMEOUT,
            |guard| {
                let Some(current) = guard.current()? else {
                    return Err(DraftError::not_found(format!(
                        "task '{}' was not found",
                        task.id
                    )));
                };
                // The caller's generation is the claim "this is the task I
                // read". Deriving the expected state from `current` instead
                // would compare the record against itself and could never
                // detect that somebody else had committed in between.
                if current.generation != task.generation {
                    return Err(DraftError::new(
                        DraftErrorKind::ConflictDetected,
                        format!(
                            "task '{}' has moved on: it is at generation {} but this edit was \
                                 made against generation {}",
                            task.id, current.generation, task.generation
                        ),
                    )
                    .with_suggestion(
                        "Re-read the task and reapply the change against its current state.",
                    ));
                }
                let expected = crate::support::record_guard::ExpectedRecordState::of(&current)?;
                let mut next = task.clone();
                next.generation = current.generation + 1;
                guard.compare_exchange_locked(&expected, &next)
            },
        )?;
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
    #[test]
    fn a_stale_task_edit_is_a_conflict_rather_than_a_silent_overwrite() {
        // Two callers read the same task, both edit it, both save. Without a
        // compare-exchange the second write wins and the first person's change
        // vanishes with no error anywhere — the failure mode where someone's
        // work disappears and nothing reports it.
        let directory = tempfile::tempdir().unwrap();
        let store = TaskStore::for_root(directory.path());
        let task = TaskDefinition::new(
            "shared".into(),
            "do the thing".into(),
            "head".into(),
            "act_test".into(),
        )
        .unwrap();
        store.create(&task).unwrap();

        let first = store.resolve("shared").unwrap().unwrap();
        let second = store.resolve("shared").unwrap().unwrap();
        assert_eq!(first.generation, second.generation);

        let mut mine = first;
        mine.goal = "my edit".into();
        store.update(&mine).unwrap();

        let mut theirs = second;
        theirs.goal = "their edit".into();
        let error = store.update(&theirs).unwrap_err();
        assert_eq!(
            error.kind,
            DraftErrorKind::ConflictDetected,
            "a stale edit must be refused, not applied over the newer one"
        );

        assert_eq!(
            store.resolve("shared").unwrap().unwrap().goal,
            "my edit",
            "the committed edit survives the refused one"
        );
    }

    #[test]
    fn creating_the_same_task_twice_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let store = TaskStore::for_root(directory.path());
        let task = TaskDefinition::new(
            "once".into(),
            "do the thing".into(),
            "head".into(),
            "act_test".into(),
        )
        .unwrap();
        store.create(&task).unwrap();
        assert!(store.create(&task).is_err());
    }

    use super::TaskLifecycleStatus;

    #[test]
    fn only_completed_and_cancelled_are_finished() {
        assert!(TaskLifecycleStatus::Completed.is_finished());
        assert!(TaskLifecycleStatus::Cancelled.is_finished());

        // Blocked work is stalled, not finished. Counting it as closed would
        // drop it out of every "still open" view, which is precisely where the
        // person who can unblock it would look.
        for open in [
            TaskLifecycleStatus::Open,
            TaskLifecycleStatus::InProgress,
            TaskLifecycleStatus::Blocked,
        ] {
            assert!(!open.is_finished(), "{open:?} is not a finished task");
        }
    }

    use super::*;

    fn contributed(
        steps: Vec<draft_extension_contract::TaskTemplateStep>,
    ) -> draft_extension_contract::TaskTemplateContribution {
        draft_extension_contract::TaskTemplateContribution {
            template_id: draft_extension_contract::NamespacedId::parse(
                "draft.software.project/fix-defect",
            )
            .unwrap(),
            display_name: "Fix a defect".into(),
            description: None,
            intent: None,
            required_evidence: Vec::new(),
            review_questions: vec!["What proves the defect is gone?".into()],
            success_criteria: vec!["A test fails before and passes after.".into()],
            steps,
        }
    }

    #[test]
    fn a_contributed_template_becomes_the_shape_core_works_with() {
        let template = resolve_template(&contributed(vec![
            draft_extension_contract::TaskTemplateStep {
                id: "reproduce".into(),
                label: "Reproduce the defect".into(),
                scope: None,
            },
            draft_extension_contract::TaskTemplateStep {
                id: "prove".into(),
                label: "Prove it with a test".into(),
                scope: Some(draft_extension_contract::ResourcePredicate::Raw {
                    of: draft_extension_contract::RawResourcePredicate::PathGlob {
                        glob: "tests/**".into(),
                    },
                }),
            },
        ]))
        .unwrap();

        assert_eq!(template.id, "draft.software.project/fix-defect");
        assert_eq!(template.name, "Fix a defect");
        assert_eq!(template.success_criteria_shape.len(), 1);
        assert_eq!(template.default_review_questions.len(), 1);
        // Each step becomes a decomposition rule; a scoped step narrows the
        // child to exactly what the contributor named.
        assert_eq!(template.decomposition_rules.len(), 2);
        assert!(template.decomposition_rules[0].zones.is_empty());
        assert_eq!(template.decomposition_rules[1].zones, vec!["tests/**"]);
        // Prohibition is project policy, not a template's parallel list.
        assert!(template.default_forbidden_zones.is_empty());
    }

    #[test]
    fn a_scope_a_zone_cannot_express_is_refused_rather_than_dropped() {
        // Silently unscoping the child would *widen* what an agent may touch,
        // which is the opposite of what scoping a step is for.
        let error = resolve_template(&contributed(vec![
            draft_extension_contract::TaskTemplateStep {
                id: "classify".into(),
                label: "Handle every source resource".into(),
                scope: Some(draft_extension_contract::ResourcePredicate::HasClass {
                    class_id: draft_extension_contract::NamespacedId::parse(
                        "draft.language.rust/source",
                    )
                    .unwrap(),
                }),
            },
        ]))
        .unwrap_err();
        assert!(
            error.message.contains("classify") && error.message.contains("path glob"),
            "{}",
            error.message
        );
    }

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
