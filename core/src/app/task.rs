//! Tasks: planning, candidates, execution and their records.
//!
//! Split out of `app/mod.rs`; these are `App` methods and behave
//! identically to when they lived there.

use super::*;

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn task_create(
        &self,
        cwd: &Path,
        name: &str,
        goal: &str,
        template: Option<String>,
        allowed_zones: Vec<String>,
        forbidden_zones: Vec<String>,
        success_criteria: Vec<String>,
        risk: Option<&str>,
        mode: Option<&str>,
        candidate_preset: Option<String>,
    ) -> DraftResult<crate::task::TaskDefinition> {
        let ws = self.open(cwd)?;
        let stable = self.accepted_baseline_ref(&ws)?;
        let actor = format!("{:?}", resolve_actor(&ws.layout.draft_dir)?);
        let mut task =
            crate::task::TaskDefinition::new(name.to_string(), goal.to_string(), stable, actor)?;
        if let Some(template) = template {
            crate::task::apply_template(&mut task, &self.resolve_task_template(&template)?)?;
        }
        if !allowed_zones.is_empty() {
            task.allowed_zones = allowed_zones;
        }
        if !forbidden_zones.is_empty() {
            task.forbidden_zones = forbidden_zones;
            if !task.forbidden_zones.iter().any(|p| p == ".draft/**") {
                task.forbidden_zones.push(".draft/**".into());
            }
        }
        if !success_criteria.is_empty() {
            task.success_criteria = success_criteria;
        }
        task.risk = match risk.unwrap_or("medium") {
            "low" => crate::task::TaskRisk::Low,
            "high" => crate::task::TaskRisk::High,
            "critical" => crate::task::TaskRisk::Critical,
            _ => crate::task::TaskRisk::Medium,
        };
        task.mode = match mode.unwrap_or("normal") {
            "safe" => crate::task::TaskMode::Safe,
            "plan-first" => crate::task::TaskMode::PlanFirst,
            _ => crate::task::TaskMode::Normal,
        };
        if let Some(candidate_preset) = candidate_preset {
            self.candidate_registry_for(&ws)?
                .preset(&candidate_preset)?;
            task.candidate_preset = Some(candidate_preset);
        }
        validate_task_definition(&self.protections(&ws.root)?, &task)?;
        crate::task::TaskStore::for_root(&ws.root).create(&task)?;
        ws.events()?.append(
            crate::activity::EventKind::TaskCreated,
            Some(task.id.to_string()),
            serde_json::to_value(&task).expect("Draft-owned records must serialize"),
        )?;
        Ok(task)
    }

    pub fn task_list(&self, cwd: &Path) -> DraftResult<Vec<crate::task::TaskDefinition>> {
        let ws = self.open(cwd)?;
        crate::task::TaskStore::for_root(&ws.root).list()
    }

    pub fn task_show(
        &self,
        cwd: &Path,
        id_or_name: &str,
    ) -> DraftResult<crate::task::TaskDefinition> {
        let ws = self.open(cwd)?;
        crate::task::TaskStore::for_root(&ws.root)
            .resolve(id_or_name)?
            .ok_or_else(|| DraftError::not_found(format!("task '{id_or_name}' was not found")))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn task_update(
        &self,
        cwd: &Path,
        id_or_name: &str,
        status: Option<crate::task::TaskLifecycleStatus>,
        priority: Option<crate::task::TaskPriority>,
        due_at: Option<Option<crate::support::common::Timestamp>>,
        assignee_ref: Option<Option<crate::task::AssigneeRef>>,
    ) -> DraftResult<crate::task::TaskDefinition> {
        let ws = self.open(cwd)?;
        let store = crate::task::TaskStore::for_root(&ws.root);
        let mut task = store
            .resolve(id_or_name)?
            .ok_or_else(|| DraftError::not_found(format!("task '{id_or_name}' was not found")))?;
        // A lifecycle transition is a different fact from an edit. Folding
        // "this task is finished" into a generic `task.updated` would make the
        // two indistinguishable to anything reading the ledger, which is
        // exactly what the frozen vocabulary separates.
        let previous_status = task.status;
        if let Some(status) = status {
            task.status = status;
        }
        if let Some(priority) = priority {
            task.priority = priority;
        }
        if let Some(due_at) = due_at {
            task.due_at = due_at;
        }
        if let Some(assignee_ref) = assignee_ref {
            if let Some(assignee) = &assignee_ref {
                if !matches!(assignee.kind.as_str(), "actor" | "candidate")
                    || assignee.id.trim().is_empty()
                {
                    return Err(DraftError::invalid_config(
                        "assignee must be a stable actor or candidate reference",
                    ));
                }
            }
            task.assignee_ref = assignee_ref;
        }
        task.updated_at = now();
        store.update(&task)?;
        let was_finished = previous_status.is_finished();
        let is_finished = task.status.is_finished();
        let event = match (was_finished, is_finished) {
            (false, true) => crate::activity::EventKind::TaskClosed,
            (true, false) => crate::activity::EventKind::TaskReopened,
            _ => crate::activity::EventKind::TaskUpdated,
        };
        ws.events()?.append(
            event,
            Some(task.id.to_string()),
            serde_json::to_value(&task).expect("Draft-owned records must serialize"),
        )?;
        Ok(task)
    }

    pub fn task_add_next_action(
        &self,
        cwd: &Path,
        id_or_name: &str,
        label: &str,
    ) -> DraftResult<crate::task::TaskDefinition> {
        let ws = self.open(cwd)?;
        let store = crate::task::TaskStore::for_root(&ws.root);
        let mut task = store
            .resolve(id_or_name)?
            .ok_or_else(|| DraftError::not_found(format!("task '{id_or_name}' was not found")))?;
        if label.trim().is_empty() {
            return Err(DraftError::invalid_config(
                "next action label cannot be empty",
            ));
        }
        task.next_actions.push(crate::task::NextAction {
            id: format!("actn_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
            label: label.trim().into(),
            completed: false,
        });
        task.updated_at = now();
        store.update(&task)?;
        ws.events()?.append(
            crate::activity::EventKind::TaskUpdated,
            Some(task.id.to_string()),
            serde_json::to_value(&task).expect("Draft-owned records must serialize"),
        )?;
        Ok(task)
    }

    pub fn task_set_next_action(
        &self,
        cwd: &Path,
        id_or_name: &str,
        action_id: &str,
        completed: bool,
    ) -> DraftResult<crate::task::TaskDefinition> {
        let ws = self.open(cwd)?;
        let store = crate::task::TaskStore::for_root(&ws.root);
        let mut task = store
            .resolve(id_or_name)?
            .ok_or_else(|| DraftError::not_found(format!("task '{id_or_name}' was not found")))?;
        let action = task
            .next_actions
            .iter_mut()
            .find(|action| action.id == action_id)
            .ok_or_else(|| {
                DraftError::not_found(format!("next action '{action_id}' was not found"))
            })?;
        action.completed = completed;
        task.updated_at = now();
        store.update(&task)?;
        ws.events()?.append(
            crate::activity::EventKind::TaskUpdated,
            Some(task.id.to_string()),
            serde_json::to_value(&task).expect("Draft-owned records must serialize"),
        )?;
        Ok(task)
    }

    pub fn task_view(&self, cwd: &Path, id_or_name: &str) -> DraftResult<crate::task::TaskView> {
        let ws = self.open(cwd)?;
        let task = crate::task::TaskStore::for_root(&ws.root)
            .resolve(id_or_name)?
            .ok_or_else(|| DraftError::not_found(format!("task '{id_or_name}' was not found")))?;
        let executions = crate::task::ExecutionStore::for_root(&ws.root).list_for_task(&task.id)?;
        let latest_execution = executions.last().map(|e| crate::task::ExecutionView {
            execution_id: e.id.to_string(),
            candidate: e.candidate.clone(),
            status: execution_status_label(e.status).to_string(),
            produced_change: e.produced_change.clone(),
            error: e
                .failure_reason
                .clone()
                .or_else(|| e.cancellation_reason.clone()),
            note: None,
        });
        let produced_changes = executions
            .iter()
            .filter_map(|e| e.produced_change.clone())
            .collect::<Vec<_>>();
        let failed = executions
            .iter()
            .filter(|e| matches!(e.status, crate::task::ExecutionStatus::Failed))
            .count();
        let running = executions
            .iter()
            .filter(|e| {
                matches!(
                    e.status,
                    crate::task::ExecutionStatus::Queued
                        | crate::task::ExecutionStatus::Running
                        | crate::task::ExecutionStatus::Retrying
                )
            })
            .count();
        // Counted from the graph: Evidence binds an exact revision, and an
        // approving Decision is the one that cites a satisfied gate over that
        // exact revision. Anything looser would report a task as reviewed on
        // the strength of a judgement about different work.
        let mut task_evidence = 0usize;
        let mut approved_changes = 0usize;
        for view in self.dcg_changes(&ws.root)? {
            if !produced_changes.iter().any(|id| id == view.change.as_str()) {
                continue;
            }
            let Some(revision) = view.revisions.first() else {
                continue;
            };
            let authorization =
                self.dcg_authorization(&ws.root, view.change.as_str(), revision.id.as_str())?;
            task_evidence += authorization.evidence.len();
            if authorization.approving_decision().is_some() {
                approved_changes += 1;
            }
        }
        let health = if failed > 0 {
            crate::task::TaskViewStatus::Blocked
        } else if running > 0 {
            crate::task::TaskViewStatus::Running
        } else if produced_changes.is_empty() {
            crate::task::TaskViewStatus::Defined
        } else if approved_changes > 0 {
            crate::task::TaskViewStatus::Approved
        } else {
            crate::task::TaskViewStatus::NeedsReview
        };
        let review_status = if approved_changes > 0 {
            crate::task::TaskViewStatus::Approved
        } else if produced_changes.is_empty() {
            crate::task::TaskViewStatus::Pending
        } else {
            crate::task::TaskViewStatus::NeedsReview
        };
        let recommended_action = if let Some(change) = produced_changes.last() {
            if approved_changes > 0 {
                // Promotion is the only step that changes what the project
                // accepts, and it names the exact revision it accepts.
                format!("draft promote {change} <rev-id>")
            } else {
                format!("draft change gates list {change} <rev-id>")
            }
        } else if running > 0 {
            format!("draft task {id_or_name} --executions")
        } else {
            format!("draft task spawn {} -c <candidate>", task.name)
        };
        Ok(crate::task::TaskView {
            task,
            health,
            latest_execution,
            review_status,
            recommended_action,
            execution_count: executions.len(),
            evidence_count: task_evidence,
            produced_changes,
        })
    }

    pub fn task_view_with_options(
        &self,
        cwd: &Path,
        id_or_name: &str,
        options: TaskViewOptions,
    ) -> DraftResult<Value> {
        let ws = self.open(cwd)?;
        let task = crate::task::TaskStore::for_root(&ws.root)
            .resolve(id_or_name)?
            .ok_or_else(|| DraftError::not_found(format!("task '{id_or_name}' was not found")))?;
        let mut out = serde_json::to_value(self.task_view(cwd, id_or_name)?)?;
        let Some(map) = out.as_object_mut() else {
            return Ok(out);
        };
        let include_all = options.full;
        let exec_store = crate::task::ExecutionStore::for_root(&ws.root);
        let executions = exec_store.list_for_task(&task.id)?;
        let produced_changes = executions
            .iter()
            .filter_map(|execution| execution.produced_change.clone())
            .collect::<Vec<_>>();

        if include_all || options.executions {
            map.insert("executions".to_string(), serde_json::to_value(&executions)?);
        }
        if include_all || options.evidence {
            // The Evidence recorded against the revisions this task produced.
            // Evidence binds an exact ChangeRevisionId, so there is nothing to
            // match loosely on and nothing that carries from another revision.
            let mut evidence = Vec::new();
            for view in self.dcg_changes(&ws.root)? {
                if !produced_changes.iter().any(|id| id == view.change.as_str()) {
                    continue;
                }
                let Some(revision) = view.revisions.first() else {
                    continue;
                };
                evidence.extend(
                    self.dcg_authorization(&ws.root, view.change.as_str(), revision.id.as_str())?
                        .evidence,
                );
            }
            map.insert("evidence".to_string(), serde_json::to_value(evidence)?);
        }
        if include_all || options.lanes {
            let lanes = executions
                .iter()
                .map(|execution| {
                    serde_json::json!({
                        "candidate": execution.candidate.clone(),
                        "execution_id": execution.id.to_string(),
                        "status": execution_status_label(execution.status),
                        "produced_change": execution.produced_change.clone(),
                        "attempt": execution.attempt,
                    })
                })
                .collect::<Vec<_>>();
            map.insert("lanes".to_string(), Value::Array(lanes));
        }
        if include_all || options.timeline {
            let task_id = task.id.to_string();
            let execution_ids = executions
                .iter()
                .map(|execution| execution.id.to_string())
                .collect::<BTreeSet<_>>();
            let change_ids = produced_changes.iter().cloned().collect::<BTreeSet<_>>();
            let events = ws
                .events()?
                .read_all()?
                .into_iter()
                .filter(|event| {
                    event
                        .subject
                        .as_ref()
                        .map(|id| {
                            id == &task_id || execution_ids.contains(id) || change_ids.contains(id)
                        })
                        .unwrap_or(false)
                })
                .collect::<Vec<_>>();
            map.insert("timeline".to_string(), serde_json::to_value(events)?);
        }
        if include_all || options.explain {
            map.insert(
                "explain".to_string(),
                serde_json::json!({
                    "template": task.template.clone(),
                    "required_evidence": task.required_evidence.clone(),
                    "review_questions": task.review_questions.clone(),
                    "next_action": map.get("recommended_action").cloned().expect("Draft-owned records must serialize"),
                }),
            );
        }
        if include_all || options.decompose {
            let children = self.task_decompose(&ws, &task)?;
            map.insert(
                "decomposition".to_string(),
                serde_json::json!({
                    "created_or_existing": children,
                    "next_action": "inspect child tasks, then spawn the candidate lane for each child task",
                }),
            );
        }
        Ok(out)
    }

    fn task_decompose(
        &self,
        ws: &Workspace,
        task: &crate::task::TaskDefinition,
    ) -> DraftResult<Vec<crate::task::TaskDefinition>> {
        let Some(template) = task.template.as_deref() else {
            return Ok(Vec::new());
        };
        let template = self.resolve_task_template(template)?;
        let store = crate::task::TaskStore::for_root(&ws.root);
        let mut children = Vec::new();
        for rule in template.decomposition_rules {
            let name = format!("{}-{}", task.name, rule.id);
            if let Some(existing) = store.resolve(&name)? {
                children.push(existing);
                continue;
            }
            let mut child = crate::task::TaskDefinition::new(
                name,
                format!("{}: {}", task.goal, rule.description),
                task.base_baseline.clone(),
                task.created_by.clone(),
            )?;
            child.kind = crate::task::TaskKind::Generated;
            child.template = rule.child_template.clone();
            child.allowed_zones = rule.zones.clone();
            child.forbidden_zones = task
                .allowed_zones
                .iter()
                .filter(|zone| !child.allowed_zones.iter().any(|allowed| allowed == *zone))
                .cloned()
                .chain(task.forbidden_zones.iter().cloned())
                .collect();
            child.required_evidence = task.required_evidence.clone();
            child.review_questions = task.review_questions.clone();
            child.candidate_preset = task.candidate_preset.clone();
            child.parent_change = task.parent_change.clone();
            child.metadata.insert(
                "parent_task".to_string(),
                Value::String(task.id.to_string()),
            );
            child.metadata.insert(
                "decomposition_rule".to_string(),
                Value::String(rule.id.clone()),
            );
            store.create(&child)?;
            ws.events()?.append(
                crate::activity::EventKind::TaskCreated,
                Some(child.id.to_string()),
                serde_json::json!({
                    "parent_task": task.id.to_string(),
                    "rule": rule.id,
                    "task_name": child.name.clone(),
                }),
            )?;
            children.push(child);
        }
        Ok(children)
    }

    pub fn task_drop(
        &self,
        cwd: &Path,
        id_or_name: &str,
        hard: bool,
    ) -> DraftResult<crate::task::TaskDropOutcome> {
        let ws = self.open(cwd)?;
        let recovery = crate::execution::operation::RecoveryStore::for_root(&ws.root);
        let entry = recovery.start(
            if hard { "task.drop_hard" } else { "task.drop" },
            Some(id_or_name.to_string()),
            serde_json::json!({ "task": id_or_name, "hard": hard }),
        )?;
        let entry = recovery.mark_in_progress(entry, None)?;
        let outcome = match crate::task::TaskStore::for_root(&ws.root).drop_task(id_or_name, hard) {
            Ok(outcome) => outcome,
            Err(err) => {
                let _ = recovery.fail(entry, err.message.clone());
                return Err(err);
            }
        };
        let _ = recovery.complete(
            entry,
            serde_json::json!({
                "task_id": outcome.task_id.clone(),
                "definition_removed": outcome.definition_removed,
                "removed_executions": outcome.removed_executions.clone(),
            }),
        )?;
        // Both forms close the task; `hard` says whether the definition went
        // with it, and the outcome payload below carries that. The vocabulary
        // has one event for "this task is closed", because a reader asking
        // what happened is asking the same question either way.
        ws.events()?.append(
            crate::activity::EventKind::TaskClosed,
            Some(outcome.task_id.clone()),
            serde_json::to_value(&outcome).expect("Draft-owned records must serialize"),
        )?;
        Ok(outcome)
    }

    pub fn task_export(
        &self,
        cwd: &Path,
        id_or_name: &str,
        output: Option<&Path>,
    ) -> DraftResult<TaskExportReport> {
        let ws = self.open(cwd)?;
        let store = crate::task::TaskStore::for_root(&ws.root);
        let task = store
            .resolve(id_or_name)?
            .ok_or_else(|| DraftError::not_found(format!("task '{id_or_name}' was not found")))?;
        let output = output
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(format!("{}.task.json", task.name)));
        let exported = store.export_to(task.id.as_str(), &output)?;
        ws.events()?.append(
            crate::activity::EventKind::TaskUpdated,
            Some(exported.id.to_string()),
            serde_json::json!({
                "task_id": exported.id.to_string(),
                "task_name": exported.name,
                "output": output.display().to_string(),
            }),
        )?;
        Ok(TaskExportReport {
            task_id: exported.id.to_string(),
            task_name: exported.name,
            output: output.display().to_string(),
            next_action: "import with `draft task import <path>` in another Draft workspace"
                .to_string(),
        })
    }

    pub fn task_import(
        &self,
        cwd: &Path,
        source: &Path,
        name: Option<String>,
    ) -> DraftResult<TaskImportReport> {
        let ws = self.open(cwd)?;
        let mut task: crate::task::TaskDefinition =
            crate::contracts::decode_wire(&fs::read(source)?)?;
        let store = crate::task::TaskStore::for_root(&ws.root);
        if let Some(name) = name {
            task.name = name;
        }
        if store.resolve(&task.name)?.is_some() {
            return Err(DraftError::new(
                DraftErrorKind::TaskDefinitionConflict,
                format!("task '{}' already exists", task.name),
            )
            .with_suggestion("pass `--name <new-name>` or drop the existing task first"));
        }
        let stable = self
            .accepted_baseline_ref(&ws)
            .unwrap_or_else(|_| "uninitialized".to_string());
        let actor = format!("{:?}", resolve_actor(&ws.layout.draft_dir)?);
        let at = now();
        task.schema_version = current_version(ContractId::TaskDefinition);
        task.id = TaskId::generate();
        task.kind = crate::task::TaskKind::Imported;
        task.created_at = at;
        task.updated_at = at;
        task.created_by = actor;
        task.base_baseline = stable;
        task.source_context = Some(crate::task::TaskSourceContext {
            locator: ResourceLocator::file(source.display().to_string()),
            coordinate_space: None,
            start: None,
            length: None,
            element_id: None,
            reason: Some("task import".to_string()),
        });
        store.import(&task)?;
        ws.events()?.append(
            crate::activity::EventKind::TaskCreated,
            Some(task.id.to_string()),
            serde_json::json!({
                "task_id": task.id.to_string(),
                "task_name": task.name,
                "source": source.display().to_string(),
            }),
        )?;
        Ok(TaskImportReport {
            task_id: task.id.to_string(),
            task_name: task.name,
            source: source.display().to_string(),
            next_action: format!("draft task spawn {}", task.id),
        })
    }

    pub fn task_retry_execution(
        &self,
        cwd: &Path,
        execution_id: &str,
    ) -> DraftResult<crate::task::Execution> {
        let ws = self.open(cwd)?;
        let execution = crate::task::ExecutionStore::for_root(&ws.root).retry(execution_id)?;
        ws.events()?.append(
            crate::activity::EventKind::OperationReplanned,
            Some(execution.id.to_string()),
            serde_json::to_value(&execution).expect("Draft-owned records must serialize"),
        )?;
        Ok(execution)
    }

    pub fn task_cancel_execution(
        &self,
        cwd: &Path,
        execution_id: &str,
        reason: Option<String>,
    ) -> DraftResult<crate::task::Execution> {
        let ws = self.open(cwd)?;
        let reason = reason.unwrap_or_else(|| "cancelled by user".to_string());
        let store = crate::task::ExecutionStore::for_root(&ws.root);
        let before = store.read(execution_id)?;
        if let Some(pid) = before.pid {
            let _ = terminate_process(pid);
        }
        let execution = store.mark_cancelled(execution_id, &reason)?;
        ws.events()?.append(
            crate::activity::EventKind::OperationRefused,
            Some(execution.id.to_string()),
            serde_json::json!({ "reason": reason }),
        )?;
        Ok(execution)
    }

    pub fn task_resume_execution(
        &self,
        cwd: &Path,
        execution_id: &str,
    ) -> DraftResult<crate::task::Execution> {
        let ws = self.open(cwd)?;
        let store = crate::task::ExecutionStore::for_root(&ws.root);
        let execution = store.read(execution_id)?;
        if !execution.is_resumable() {
            return Err(DraftError::invalid_config(format!(
                "execution {execution_id} is not resumable"
            )));
        }
        let registry = self.candidate_registry_for(&ws)?;
        registry
            .profile(&execution.candidate)?
            .ensure_capability("resume")?;
        let resumed = store.update(execution_id, |e| {
            e.status = crate::task::ExecutionStatus::Queued;
            e.finished_at = None;
            e.failure_reason = None;
            e.cancellation_reason = None;
        })?;
        ws.events()?.append(
            crate::activity::EventKind::OperationReplanned,
            Some(resumed.id.to_string()),
            serde_json::to_value(&resumed).expect("Draft-owned records must serialize"),
        )?;
        Ok(resumed)
    }

    #[allow(clippy::too_many_arguments)]
    /// Create a task scoped to a region of one resource.
    ///
    /// The region is expressed in a contributed coordinate space — lines of a
    /// document, keys of a record set, frames of a timeline. Draft stores the
    /// coordinates and never interprets them.
    pub fn task_create_from_selection(
        &self,
        cwd: &Path,
        locator: &ResourceLocator,
        coordinate_space: &str,
        start: u64,
        length: u64,
        selected_text: &str,
        reason: Option<String>,
        workspace_hash: Option<String>,
    ) -> DraftResult<ResourceSelectionTaskReport> {
        let ws = self.open(cwd)?;
        let rel = checked_resource_path(&self.protections(&ws.root)?, locator)?;
        let current_hash = self.workspace_hash(&ws.root)?;
        if let Some(expected) = workspace_hash {
            if !expected.is_empty() && expected != current_hash {
                return Err(DraftError::new(
                    DraftErrorKind::DirtyWorkspace,
                    "the project changed since this selection was read",
                )
                .with_suggestion("reload the resource before creating a task from selection"));
            }
        }
        let stable = self.accepted_baseline_ref(&ws)?;
        let actor = format!("{:?}", resolve_actor(&ws.layout.draft_dir)?);
        let mut task = crate::task::TaskDefinition::new(
            format!("Review {}", rel.as_str()),
            reason
                .clone()
                .unwrap_or_else(|| format!("Review the selected region of {}", rel.as_str())),
            stable,
            actor,
        )?;
        task.kind = crate::task::TaskKind::Defined;
        task.source_context = Some(crate::task::TaskSourceContext {
            locator: locator.clone(),
            coordinate_space: Some(coordinate_space.to_string()),
            start: Some(start),
            length: Some(length),
            element_id: None,
            reason,
        });
        task.allowed_zones = vec![rel.to_string()];
        task.success_criteria =
            vec!["The selected region has been reviewed and addressed".to_string()];
        task.metadata.insert(
            "selected_text".to_string(),
            serde_json::Value::String(crate::support::redaction::redact(selected_text)),
        );
        task.metadata.insert(
            "selection_workspace_hash".to_string(),
            serde_json::Value::String(current_hash),
        );
        crate::task::TaskStore::for_root(&ws.root).create(&task)?;
        ws.events()?.append(
            crate::activity::EventKind::TaskCreated,
            Some(task.id.to_string()),
            serde_json::to_value(&task).expect("Draft-owned records must serialize"),
        )?;
        Ok(ResourceSelectionTaskReport {
            task_id: task.id.to_string(),
            locator: locator.clone(),
            coordinate_space: coordinate_space.to_string(),
            start,
            length,
        })
    }

    /// The canonical spawn engine.
    ///
    /// See `docs/reference/commands.md` on `draft task spawn`.
    ///
    /// Resolves a stored task (or creates an inline one), resolves the
    /// candidate list or preset, validates capabilities, and runs one real
    /// execution per candidate in an isolated workspace. Each successful
    /// execution produces a change diffed against the same pre-spawn baseline;
    /// the working tree is left exactly as it was before the spawn.
    #[allow(clippy::too_many_arguments)]
    pub fn task_spawn(
        &self,
        cwd: &Path,
        name: &str,
        change_id: Option<&str>,
        candidates: Vec<String>,
        cron: Option<String>,
        instruction: Vec<String>,
    ) -> DraftResult<TaskSpawnReport> {
        self.task_spawn_with_preset(cwd, name, change_id, candidates, None, cron, instruction)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn task_spawn_with_preset(
        &self,
        cwd: &Path,
        name: &str,
        change_id: Option<&str>,
        mut candidates: Vec<String>,
        preset: Option<String>,
        cron: Option<String>,
        instruction: Vec<String>,
    ) -> DraftResult<TaskSpawnReport> {
        let ws = self.open(cwd)?;
        let store = crate::task::TaskStore::for_root(&ws.root);
        let exec_store = crate::task::ExecutionStore::for_root(&ws.root);
        let instruction = instruction.join(" ");

        // Stored-vs-inline instruction rules.
        let mut task = match store.resolve(name)? {
            Some(stored) => {
                if !instruction.trim().is_empty() {
                    return Err(DraftError::new(
                        DraftErrorKind::TaskDefinitionConflict,
                        format!(
                            "task '{name}' already has a stored definition; spawn it without an inline instruction"
                        ),
                    )
                    .with_suggestion(format!(
                        "run `draft task spawn {name}` to use the stored goal, or `draft task {name}` to inspect it"
                    )));
                }
                stored
            }
            None => {
                if instruction.trim().is_empty() {
                    return Err(DraftError::invalid_config(format!(
                        "no stored task named '{name}'; an inline instruction is required"
                    ))
                    .with_suggestion(format!(
                        "run `draft task spawn {name} -- <instruction>` or create it first with `draft task create {name} --goal <goal>`"
                    )));
                }
                let stable = self.accepted_baseline_ref(&ws)?;
                let actor = format!("{:?}", resolve_actor(&ws.layout.draft_dir)?);
                let task_name = inline_task_name(name);
                let mut t = crate::task::TaskDefinition::new(
                    task_name,
                    instruction.clone(),
                    stable,
                    actor,
                )?;
                t.kind = crate::task::TaskKind::Inline;
                store.create(&t)?;
                ws.events()?.append(
                    crate::activity::EventKind::TaskCreated,
                    Some(t.id.to_string()),
                    serde_json::to_value(&t).expect("Draft-owned records must serialize"),
                )?;
                t
            }
        };

        if let Some(cron) = cron {
            task.schedule = Some(crate::task::TaskSchedule {
                cron: Some(cron),
                note: None,
            });
        }

        // Candidate list / preset resolution.
        let registry = self.candidate_registry_for(&ws)?;
        let mut preset_used = None;
        if candidates.is_empty() {
            let preset_name = preset.clone().or_else(|| task.candidate_preset.clone());
            if let Some(preset_name) = preset_name {
                let p = registry.preset(&preset_name)?;
                candidates = p.candidates.clone();
                preset_used = Some(p);
            } else {
                candidates.push("manual".to_string());
            }
        } else if let Some(preset_name) = preset {
            // Explicit candidates win, but a named preset still applies its policy.
            preset_used = Some(registry.preset(&preset_name)?);
        }
        if let Some(p) = &preset_used {
            if p.plan_first && task.mode == crate::task::TaskMode::Normal {
                task.mode = crate::task::TaskMode::PlanFirst;
            }
            if p.require_full_evidence && !task.required_evidence.iter().any(|e| e == "full_tests")
            {
                task.required_evidence.push("full_tests".to_string());
            }
            if p.prefer_smallest_valid_change {
                task.metadata.insert(
                    "prefer_smallest_valid_change".to_string(),
                    Value::Bool(true),
                );
            }
        }
        task.updated_at = now();
        store.update(&task)?;

        // Validate every candidate profile before starting any execution.
        let mut profiles = Vec::new();
        for candidate in &candidates {
            let profile = registry.profile(candidate)?;
            if profile.kind.runs_command() {
                profile.ensure_capability("edit")?;
                if task.mode == crate::task::TaskMode::PlanFirst {
                    profile.ensure_capability("plan")?;
                }
                if profile.command.is_none() {
                    return Err(DraftError::new(
                        DraftErrorKind::CandidateNotConfigured,
                        format!("candidate '{candidate}' has no command configured"),
                    )
                    .with_suggestion(format!(
                        "set `command` under [candidates.{candidate}] in .draft/config.toml"
                    )));
                }
            }
            profiles.push(profile);
        }
        if profiles.iter().any(|profile| profile.kind.runs_command()) {
            crate::app::baseline::require_workspace_matches_baseline(
                self,
                &ws,
                "task spawn",
                "create a Change from the current edits, discard them, or run the task from a \
                 workspace matching the accepted Baseline",
            )?;
        }

        let parent_change = Some(match change_id {
            Some(change_id) => change_id.to_string(),
            None => self.selected_change_id(cwd)?,
        });
        ws.events()?.append(
            crate::activity::EventKind::TaskUpdated,
            Some(task.id.to_string()),
            serde_json::json!({
                "change_id": parent_change,
                "candidates": candidates,
                "preset": preset_used.as_ref().map(|p| p.name.clone()),
                "instruction": redact_secrets(&task.goal),
            }),
        )?;

        // One shared pre-spawn baseline: every candidate change diffs against it.
        let baseline = self.observe(&ws)?;
        let mut executions = Vec::new();
        for profile in &profiles {
            let command = profile
                .command
                .as_deref()
                .map(|t| crate::task::candidate::render_command(t, &task.goal))
                .unwrap_or_default();
            let mut execution =
                crate::task::Execution::queued(&task, profile.name.clone(), command);
            execution.workspace_id = Some(ws.workspace_id.to_string());
            execution.parent_change = parent_change.clone();
            exec_store.write(&execution)?;
            ws.events()?.append(
                crate::activity::EventKind::OperationPlanned,
                Some(execution.id.to_string()),
                serde_json::json!({
                    "task_id": task.id.to_string(),
                    "candidate": profile.name,
                }),
            )?;
            if !profile.kind.runs_command() {
                executions.push(ExecutionSummary {
                    execution_id: execution.id.to_string(),
                    candidate: profile.name.clone(),
                    status: "queued".to_string(),
                    produced_change: None,
                    error: None,
                    note: Some(
                        "human execution: make edits in the editor or workspace, then create a Change"
                            .to_string(),
                    ),
                });
                continue;
            }
            match self.run_candidate_execution(&ws, &task, &execution, profile, &baseline) {
                Ok(produced_change) => {
                    let refreshed = exec_store.read(execution.id.as_str())?;
                    executions.push(ExecutionSummary {
                        execution_id: execution.id.to_string(),
                        candidate: profile.name.clone(),
                        status: execution_status_label(refreshed.status).to_string(),
                        produced_change,
                        error: refreshed.failure_reason,
                        note: None,
                    });
                }
                Err(e) => {
                    let _ = exec_store.mark_failed(execution.id.as_str(), &e.to_string());
                    let _ = ws.events()?.append(
                        crate::activity::EventKind::OperationRefused,
                        Some(execution.id.to_string()),
                        serde_json::json!({
                            "task_id": task.id.to_string(),
                            "candidate": profile.name,
                            "reason": redact_secrets(&e.to_string()),
                        }),
                    );
                    executions.push(ExecutionSummary {
                        execution_id: execution.id.to_string(),
                        candidate: profile.name.clone(),
                        status: "failed".to_string(),
                        produced_change: None,
                        error: Some(e.to_string()),
                        note: None,
                    });
                }
            }
        }
        store.rebuild_index()?;

        let next_action = if let Some(done) = executions
            .iter()
            .find(|e| e.status == "completed" && e.produced_change.is_some())
        {
            format!(
                "draft change review {}",
                done.produced_change.clone().unwrap_or_default()
            )
        } else if executions.iter().any(|e| e.status == "queued") {
            "make the edits, then run `draft change new` to capture them".to_string()
        } else {
            format!("draft task {} --executions", task.name)
        };
        Ok(TaskSpawnReport {
            task_id: task.id.to_string(),
            task_name: task.name.clone(),
            task_kind: format!("{:?}", task.kind).to_lowercase(),
            preset: preset_used.map(|p| p.name),
            parent_change,
            executions,
            next_action,
        })
    }

    pub fn task_current(&self, cwd: &Path) -> DraftResult<Value> {
        let tasks = self.task_list(cwd)?;
        if let Some(task) = tasks.last() {
            Ok(serde_json::to_value(task).expect("Draft-owned records must serialize"))
        } else {
            Ok(serde_json::json!({ "message": "No running tasks." }))
        }
    }

    /// Every task template the installed extensions contribute.
    pub fn task_templates(&self, cwd: &Path) -> DraftResult<Vec<crate::task::TaskTemplate>> {
        let _ = self.open(cwd)?;
        let contributions = self.active_contributions();
        let mut out = Vec::new();
        for contributed in contributions.task_templates().values() {
            out.push(crate::task::resolve_template(contributed)?);
        }
        Ok(out)
    }
}
