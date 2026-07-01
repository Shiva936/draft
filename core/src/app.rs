use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::common::{now, ActorId, ReceiptId, WorkspaceId, WorkspacePath};
use crate::error::{DraftError, DraftErrorKind, DraftResult};
use crate::fsutil::{
    ensure_dir, list_with_extension, read_json, read_toml, write_atomic, write_json, write_toml,
};
use crate::identity::{resolve_actor, ActorKind, ActorRef};
use crate::lock::FileGuard;

const DRAFT_DIR: &str = ".draft";
const SCHEMA_VERSION: u32 = 3;

crate::id_newtype!(EventId, "evt_");
crate::id_newtype!(SnapshotId, "snap_");
crate::id_newtype!(TaskId, "task_");
crate::id_newtype!(RunId, "run_");
crate::id_newtype!(ChangepackId, "pack_");
crate::id_newtype!(EvidenceId, "evd_");
crate::id_newtype!(PatchSetId, "patch_");
crate::id_newtype!(DecisionId, "dec_");
crate::id_newtype!(ReviewCommentId, "rcom_");
crate::id_newtype!(RollbackPlanId, "rbp_");

#[derive(Debug, Clone)]
pub struct App;

impl App {
    pub fn new() -> Self {
        App
    }

    pub fn init(&self, root: &Path) -> DraftResult<InitReport> {
        let layout = DraftLayout::for_root(root);
        let created = !layout.draft_dir.exists();
        layout.create_all()?;
        if !layout.config_toml().exists() {
            write_toml(&layout.config_toml(), &DraftConfig::default())?;
        }
        if !layout.ignore_file().exists() {
            write_atomic(&layout.ignore_file(), DEFAULT_IGNORE.as_bytes())?;
        }
        if !layout.verify_toml().exists() {
            write_toml(&layout.verify_toml(), &VerifyFile::default())?;
        }
        if !layout.policy_toml().exists() {
            write_toml(&layout.policy_toml(), &PolicyConfig::default())?;
        }
        rebuild_index_for_layout(&layout)?;
        let meta = if layout.workspace_json().exists() {
            read_json::<WorkspaceMetadata>(&layout.workspace_json())?
        } else {
            let meta = WorkspaceMetadata {
                schema_version: SCHEMA_VERSION,
                id: WorkspaceId::generate(),
                draft_version: crate::DRAFT_VERSION.to_string(),
                created_at: now(),
            };
            write_json(&layout.workspace_json(), &meta)?;
            meta
        };
        let store = EventStore::new(layout.clone(), meta.id.clone())?;
        if created {
            store.append(
                "WorkspaceInitialized",
                None,
                serde_json::json!({ "root": root.display().to_string() }),
            )?;
        }
        Ok(InitReport {
            workspace_id: meta.id.to_string(),
            root: root.display().to_string(),
            created,
            draft_dir: layout.draft_dir.display().to_string(),
        })
    }

    pub fn open(&self, cwd: &Path) -> DraftResult<Workspace> {
        let root = find_workspace_root(cwd).ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::WorkspaceNotFound,
                "not inside a Draft workspace",
            )
            .with_suggestion("run `draft init`")
        })?;
        let layout = DraftLayout::for_root(&root);
        let meta = read_json::<WorkspaceMetadata>(&layout.workspace_json())?;
        Ok(Workspace {
            id: meta.id,
            root,
            layout,
        })
    }

    pub fn config_set(&self, cwd: &Path, key: &str, value: &str) -> DraftResult<ConfigReport> {
        reject_remote_key(key)?;
        validate_config_key(key)?;
        let ws = self.open(cwd)?;
        let mut cfg = read_or_default::<DraftConfig>(&ws.layout.config_toml());
        cfg.set(key, value)?;
        write_toml(&ws.layout.config_toml(), &cfg)?;
        ws.events()?
            .append("ConfigChanged", None, serde_json::json!({ "key": key }))?;
        Ok(ConfigReport::single(key, value))
    }

    pub fn config_get(&self, cwd: &Path, key: &str) -> DraftResult<ConfigReport> {
        reject_remote_key(key)?;
        validate_config_key(key)?;
        let ws = self.open(cwd)?;
        let cfg = ResolvedConfig::load(&ws)?;
        Ok(ConfigReport::single(key, &cfg.get(key).unwrap_or_default()))
    }

    pub fn config_unset(&self, cwd: &Path, key: &str) -> DraftResult<ConfigReport> {
        reject_remote_key(key)?;
        validate_config_key(key)?;
        let ws = self.open(cwd)?;
        let mut cfg = read_or_default::<DraftConfig>(&ws.layout.config_toml());
        cfg.unset(key)?;
        write_toml(&ws.layout.config_toml(), &cfg)?;
        ws.events()?
            .append("ConfigChanged", None, serde_json::json!({ "key": key }))?;
        Ok(ConfigReport::single(key, ""))
    }

    pub fn config_list(&self, cwd: &Path) -> DraftResult<ConfigReport> {
        let ws = self.open(cwd)?;
        Ok(ConfigReport {
            entries: ResolvedConfig::load(&ws)?.entries(),
        })
    }

    pub fn ignore_add(&self, cwd: &Path, pattern: &str) -> DraftResult<IgnoreReport> {
        let ws = self.open(cwd)?;
        let mut patterns = read_ignore_lines(&ws.layout.ignore_file())?;
        if !patterns.iter().any(|p| p == pattern) {
            patterns.push(pattern.to_string());
            write_atomic(&ws.layout.ignore_file(), patterns.join("\n").as_bytes())?;
            ws.events()?.append(
                "IgnoreRulesChanged",
                None,
                serde_json::json!({ "action": "add", "pattern": pattern }),
            )?;
        }
        self.ignore_list(cwd)
    }

    pub fn ignore_remove(&self, cwd: &Path, pattern: &str) -> DraftResult<IgnoreReport> {
        let ws = self.open(cwd)?;
        let mut patterns = read_ignore_lines(&ws.layout.ignore_file())?;
        patterns.retain(|p| p != pattern);
        write_atomic(&ws.layout.ignore_file(), patterns.join("\n").as_bytes())?;
        ws.events()?.append(
            "IgnoreRulesChanged",
            None,
            serde_json::json!({ "action": "remove", "pattern": pattern }),
        )?;
        self.ignore_list(cwd)
    }

    pub fn ignore_list(&self, cwd: &Path) -> DraftResult<IgnoreReport> {
        let ws = self.open(cwd)?;
        Ok(IgnoreReport {
            patterns: read_ignore_lines(&ws.layout.ignore_file())?,
        })
    }

    pub fn status(&self, cwd: &Path) -> DraftResult<WorkspaceStatus> {
        let ws = self.open(cwd)?;
        let status = Scanner::new(&ws)?.status()?;
        ws.events()?.append(
            "WorkspaceScanned",
            None,
            serde_json::json!({
                "changes": status.changes.len(),
                "ignored_count": status.ignored_count
            }),
        )?;
        Ok(status)
    }

    pub fn checkpoint(&self, cwd: &Path, message: &str) -> DraftResult<CheckpointReport> {
        let ws = self.open(cwd)?;
        let snapshot = Snapshotter::new(&ws)?.create_snapshot()?;
        let receipt = Receipt::new(
            "checkpoint",
            "completed",
            Some(snapshot.id.to_string()),
            serde_json::json!({ "message": message }),
        );
        write_receipt(&ws, &receipt)?;
        ws.events()?.append(
            "SnapshotCreated",
            Some(snapshot.id.to_string()),
            serde_json::json!({ "message": message }),
        )?;
        Ok(CheckpointReport {
            snapshot_id: snapshot.id.to_string(),
            receipt_id: receipt.id.to_string(),
            files: snapshot.files.len(),
        })
    }

    pub fn task_create(
        &self,
        cwd: &Path,
        title: &str,
        description: Option<String>,
    ) -> DraftResult<Task> {
        let ws = self.open(cwd)?;
        let task = Task {
            schema_version: SCHEMA_VERSION,
            id: TaskId::generate(),
            title: title.to_string(),
            description,
            created_by: resolve_actor(&ws.layout.draft_dir),
            risk_profile: None,
            linked_issue: None,
            created_at: now(),
            status: TaskStatus::Open,
        };
        write_json(
            &ws.layout.tasks_dir().join(format!("{}.json", task.id)),
            &task,
        )?;
        ws.events()?.append(
            "TaskCreated",
            Some(task.id.to_string()),
            serde_json::to_value(&task).unwrap_or(Value::Null),
        )?;
        Ok(task)
    }

    pub fn task_list(&self, cwd: &Path) -> DraftResult<Vec<Task>> {
        let ws = self.open(cwd)?;
        load_json_dir(&ws.layout.tasks_dir())
    }

    pub fn task_show(&self, cwd: &Path, id: &str) -> DraftResult<Task> {
        let ws = self.open(cwd)?;
        read_json(&ws.layout.tasks_dir().join(format!("{}.json", id)))
    }

    pub fn pack_create(
        &self,
        cwd: &Path,
        name: Option<String>,
        task_id: Option<String>,
        from_working_tree: bool,
    ) -> DraftResult<Changepack> {
        let ws = self.open(cwd)?;
        let base = latest_snapshot(&ws)?.unwrap_or_else(|| empty_snapshot(&ws));
        let result = Snapshotter::new(&ws)?.create_snapshot()?;
        let patch = diff_snapshots(&ws, &base, &result)?;
        let evidence = Evidence {
            schema_version: SCHEMA_VERSION,
            id: EvidenceId::generate(),
            changepack_id: ChangepackId::new("pending"),
            command_logs: vec![],
            files_touched: patch.files.iter().map(|f| f.path.clone()).collect(),
            generated_diff_ref: None,
            test_results: vec![],
            lint_results: vec![],
            risk_summary_ref: None,
            agent_plan_ref: None,
            agent_transcript_ref: None,
            warnings: if from_working_tree {
                vec![]
            } else {
                vec!["created from current workspace snapshot".to_string()]
            },
            created_at: now(),
        };
        let mut pack = Changepack::new(
            ws.id.clone(),
            task_id.map(TaskId::new),
            None,
            base.id.clone(),
            result.id.clone(),
            name,
        );
        let mut evidence = evidence;
        evidence.changepack_id = pack.id.clone();
        let pack_dir = ws.layout.pack_dir(&pack.id);
        ensure_dir(&pack_dir)?;
        write_json(&pack_dir.join("manifest.json"), &pack)?;
        write_json(&pack_dir.join("patch.json"), &patch)?;
        write_json(&pack_dir.join("evidence.json"), &evidence)?;
        pack.patch_refs.push(patch.id.to_string());
        pack.evidence_refs.push(evidence.id.to_string());
        pack.manifest_hash = hash_json(&pack)?;
        write_json(&pack_dir.join("manifest.json"), &pack)?;
        ws.events()?.append(
            "ChangepackCreated",
            Some(pack.id.to_string()),
            serde_json::to_value(&pack).unwrap_or(Value::Null),
        )?;
        Ok(pack)
    }

    pub fn pack_list(&self, cwd: &Path) -> DraftResult<Vec<Changepack>> {
        let ws = self.open(cwd)?;
        let mut packs = Vec::new();
        if ws.layout.changepacks_dir().exists() {
            for entry in fs::read_dir(ws.layout.changepacks_dir())? {
                let p = entry?.path().join("manifest.json");
                if p.exists() {
                    packs.push(read_json(&p)?);
                }
            }
        }
        packs.sort_by_key(|a: &Changepack| a.created_at);
        Ok(packs)
    }

    pub fn pack_show(&self, cwd: &Path, id: &str) -> DraftResult<PackReport> {
        let ws = self.open(cwd)?;
        let pack = load_pack(&ws, id)?;
        let patch = load_patch(&ws, &pack)?;
        let evidence = load_evidence(&ws, &pack).ok();
        Ok(PackReport {
            pack,
            patch,
            evidence,
        })
    }

    pub fn spawn_run(
        &self,
        cwd: &Path,
        task_id: &str,
        name: &str,
        command: Vec<String>,
    ) -> DraftResult<Run> {
        let ws = self.open(cwd)?;
        let base = Snapshotter::new(&ws)?.create_snapshot()?;
        let run_id = RunId::generate();
        ws.events()?.append(
            "RunStarted",
            Some(run_id.to_string()),
            serde_json::json!({ "task_id": task_id, "name": name, "command": command }),
        )?;
        let started = now();
        let output = if command.is_empty() {
            return Err(DraftError::new(
                DraftErrorKind::InvalidConfig,
                "spawn command is empty",
            ));
        } else {
            Command::new(&command[0])
                .args(&command[1..])
                .current_dir(&ws.root)
                .output()
        };
        let (status, stdout_ref, stderr_ref, exit_code) = match output {
            Ok(out) => {
                let store = ObjectStore::new(ws.layout.clone());
                let stdout_ref = store.put_bytes(&out.stdout)?;
                let stderr_ref = store.put_bytes(&out.stderr)?;
                (
                    if out.status.success() {
                        RunStatus::Completed
                    } else {
                        RunStatus::Failed
                    },
                    Some(stdout_ref),
                    Some(stderr_ref),
                    out.status.code(),
                )
            }
            Err(e) => (
                RunStatus::Failed,
                None,
                None,
                Some(-1).filter(|_| {
                    let _ = e;
                    true
                }),
            ),
        };
        let result = Snapshotter::new(&ws)?.create_snapshot()?;
        let run = Run {
            schema_version: SCHEMA_VERSION,
            id: run_id,
            task_id: TaskId::new(task_id),
            workspace_id: ws.id.clone(),
            base_snapshot_id: base.id,
            actor_kind: ActorKind::Agent,
            actor_name: name.to_string(),
            command: Some(command.join(" ")),
            started_at: started,
            ended_at: Some(now()),
            status,
            stdout_ref,
            stderr_ref,
            exit_code,
            result_snapshot_id: Some(result.id),
        };
        write_json(&ws.layout.runs_dir().join(format!("{}.json", run.id)), &run)?;
        ws.events()?.append(
            "RunCompleted",
            Some(run.id.to_string()),
            serde_json::to_value(&run).unwrap_or(Value::Null),
        )?;
        let _ = self.pack_create(cwd, Some(name.to_string()), Some(task_id.to_string()), true)?;
        Ok(run)
    }

    pub fn runs(&self, cwd: &Path) -> DraftResult<Vec<Run>> {
        let ws = self.open(cwd)?;
        load_json_dir(&ws.layout.runs_dir())
    }

    pub fn run_show(&self, cwd: &Path, id: &str) -> DraftResult<Run> {
        let ws = self.open(cwd)?;
        read_json(&ws.layout.runs_dir().join(format!("{}.json", id)))
    }

    pub fn verify(&self, cwd: &Path, pack_id: &str) -> DraftResult<VerificationReport> {
        let ws = self.open(cwd)?;
        let mut pack = load_pack(&ws, pack_id)?;
        let checks = read_or_default::<VerifyFile>(&ws.layout.verify_toml()).checks;
        ws.events()?.append(
            "VerificationStarted",
            Some(pack.id.to_string()),
            serde_json::json!({ "checks": checks.len() }),
        )?;
        let store = ObjectStore::new(ws.layout.clone());
        let mut results = Vec::new();
        for check in checks
            .into_iter()
            .filter(|c| c.enabled && !c.command.trim().is_empty())
        {
            let start = Instant::now();
            let out = shell(&check.command, &ws.root);
            let (exit_code, stdout, stderr) = match out {
                Ok(o) => (o.status.code().unwrap_or(-1), o.stdout, o.stderr),
                Err(e) => (-1, Vec::new(), e.to_string().into_bytes()),
            };
            results.push(VerificationResult {
                check_name: check.name,
                command_hash: command_hash(&default_shell(), &ws.root, &check.command, ""),
                started_at: now(),
                ended_at: now(),
                duration_ms: start.elapsed().as_millis() as u64,
                exit_code,
                stdout_ref: store.put_bytes(&stdout)?,
                stderr_ref: store.put_bytes(&stderr)?,
                status: if exit_code == 0 {
                    VerificationStatus::Passed
                } else {
                    VerificationStatus::Failed
                },
            });
        }
        if results.is_empty() {
            results.push(VerificationResult::skipped());
        }
        let failed = results
            .iter()
            .any(|r| r.status == VerificationStatus::Failed);
        let receipt = Receipt::new(
            "verification",
            if failed { "failed" } else { "passed" },
            Some(pack.id.to_string()),
            serde_json::to_value(&results).unwrap_or(Value::Null),
        );
        write_receipt(&ws, &receipt)?;
        let report = VerificationReport {
            changepack_id: pack.id.to_string(),
            receipt_id: receipt.id.to_string(),
            results,
        };
        pack.verification_refs.push(receipt.id.to_string());
        if !failed && matches!(pack.status, ChangepackStatus::Draft) {
            pack.status = pack.status.transition(ChangepackStatus::Verified)?;
        }
        save_pack_manifest(&ws, &mut pack)?;
        ws.events()?.append(
            "VerificationCompleted",
            Some(pack.id.to_string()),
            serde_json::to_value(&report).unwrap_or(Value::Null),
        )?;
        Ok(report)
    }

    pub fn risk(&self, cwd: &Path, pack_id: &str) -> DraftResult<RiskSummary> {
        let ws = self.open(cwd)?;
        let pack = load_pack(&ws, pack_id)?;
        let patch = load_patch(&ws, &pack)?;
        let mut score = patch.files.len() as u32;
        let mut factors = Vec::new();
        if patch.files.iter().any(|f| f.binary) {
            score += 3;
            factors.push("binary files".to_string());
        }
        if patch
            .files
            .iter()
            .any(|f| matches!(f.change_kind, FileChangeKind::Deleted))
        {
            score += 2;
            factors.push("deletions".to_string());
        }
        if patch
            .files
            .iter()
            .any(|f| f.path.0.contains("secret") || f.path.0.contains(".env"))
        {
            score += 5;
            factors.push("sensitive paths".to_string());
        }
        let level = if score >= 10 {
            RiskLevel::Critical
        } else if score >= 6 {
            RiskLevel::High
        } else if score >= 3 {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
        };
        let summary = RiskSummary {
            changepack_id: pack.id.to_string(),
            level,
            score,
            factors,
            files_changed: patch.files.len(),
        };
        let receipt = Receipt::new(
            "risk",
            level.label(),
            Some(pack.id.to_string()),
            serde_json::to_value(&summary).unwrap_or(Value::Null),
        );
        write_receipt(&ws, &receipt)?;
        ws.events()?.append(
            "RiskAssessed",
            Some(pack.id.to_string()),
            serde_json::to_value(&summary).unwrap_or(Value::Null),
        )?;
        Ok(summary)
    }

    pub fn review(
        &self,
        cwd: &Path,
        pack_id: &str,
        comment: Option<String>,
    ) -> DraftResult<ReviewReport> {
        let ws = self.open(cwd)?;
        let mut pack = load_pack(&ws, pack_id)?;
        let mut comments = load_review_file(&ws, &pack.id).unwrap_or_default();
        if let Some(body) = comment {
            comments.comments.push(ReviewComment {
                id: ReviewCommentId::generate(),
                changepack_id: pack.id.clone(),
                path: None,
                hunk_id: None,
                actor: resolve_actor(&ws.layout.draft_dir),
                body,
                created_at: now(),
            });
            ws.events()?.append(
                "ReviewCommentAdded",
                Some(pack.id.to_string()),
                serde_json::json!({ "count": comments.comments.len() }),
            )?;
        } else {
            ws.events()?.append(
                "ReviewStarted",
                Some(pack.id.to_string()),
                serde_json::json!({}),
            )?;
        }
        if matches!(
            pack.status,
            ChangepackStatus::Draft | ChangepackStatus::Verified
        ) {
            pack.status = pack.status.transition(ChangepackStatus::Reviewed)?;
            save_pack_manifest(&ws, &mut pack)?;
        }
        save_review_file(&ws, &pack.id, &comments)?;
        Ok(ReviewReport {
            changepack_id: pack.id.to_string(),
            comments: comments.comments.len(),
            decisions: comments.decisions.len(),
            status: pack.status,
        })
    }

    pub fn decide(
        &self,
        cwd: &Path,
        pack_id: &str,
        kind: DecisionKind,
        reason: Option<String>,
    ) -> DraftResult<Decision> {
        let ws = self.open(cwd)?;
        let mut pack = load_pack(&ws, pack_id)?;
        let decision = Decision {
            id: DecisionId::generate(),
            changepack_id: pack.id.clone(),
            actor: resolve_actor(&ws.layout.draft_dir),
            kind,
            reason,
            created_at: now(),
        };
        let mut file = load_review_file(&ws, &pack.id).unwrap_or_default();
        file.decisions.push(decision.clone());
        save_review_file(&ws, &pack.id, &file)?;
        pack.decision_refs.push(decision.id.to_string());
        pack.status = match decision.kind {
            DecisionKind::Approve => pack.status.transition(ChangepackStatus::Approved)?,
            DecisionKind::Reject => pack.status.transition(ChangepackStatus::Rejected)?,
            _ => pack.status,
        };
        save_pack_manifest(&ws, &mut pack)?;
        let event = if decision.kind == DecisionKind::Approve {
            "ChangepackApproved"
        } else if decision.kind == DecisionKind::Reject {
            "ChangepackRejected"
        } else {
            "DecisionRecorded"
        };
        let receipt_kind = if decision.kind == DecisionKind::Approve {
            "approval"
        } else {
            "review"
        };
        let receipt = Receipt::new(
            receipt_kind,
            decision.kind.label(),
            Some(pack.id.to_string()),
            serde_json::to_value(&decision).unwrap_or(Value::Null),
        );
        write_receipt(&ws, &receipt)?;
        ws.events()?.append(
            event,
            Some(pack.id.to_string()),
            serde_json::to_value(&decision).unwrap_or(Value::Null),
        )?;
        Ok(decision)
    }

    pub fn compare(&self, cwd: &Path, left: &str, right: &str) -> DraftResult<CompareReport> {
        let ws = self.open(cwd)?;
        let l = load_pack(&ws, left)?;
        let r = load_pack(&ws, right)?;
        let lp = load_patch(&ws, &l)?;
        let rp = load_patch(&ws, &r)?;
        let lf: BTreeSet<_> = lp.files.iter().map(|f| f.path.clone()).collect();
        let rf: BTreeSet<_> = rp.files.iter().map(|f| f.path.clone()).collect();
        let overlapping_files: Vec<_> = lf.intersection(&rf).cloned().collect();
        let overlapping_hunks = hunk_overlaps(&lp, &rp);
        let mut warnings = Vec::new();
        for path in &overlapping_files {
            let left_file = lp.files.iter().find(|f| &f.path == path);
            let right_file = rp.files.iter().find(|f| &f.path == path);
            if let (Some(lf), Some(rf)) = (left_file, right_file) {
                if file_level_conflict(lf, rf) {
                    warnings.push(format!("{path}: non-text or whole-file overlap"));
                }
            }
        }
        if !overlapping_hunks.is_empty() {
            warnings.push(format!(
                "{} overlapping text hunk(s)",
                overlapping_hunks.len()
            ));
        }
        let compatible = warnings.is_empty();
        let report = CompareReport {
            id: format!("cmp_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
            left_pack: l.id.to_string(),
            right_pack: r.id.to_string(),
            overlapping_files,
            overlapping_hunks,
            unique_left_files: lf.difference(&rf).cloned().collect(),
            unique_right_files: rf.difference(&lf).cloned().collect(),
            compatible,
            warnings,
            recommendation: Some(if compatible {
                "compose is allowed".to_string()
            } else {
                "resolve overlaps before compose".to_string()
            }),
        };
        ws.events()?.append(
            "ChangepackCompared",
            None,
            serde_json::to_value(&report).unwrap_or(Value::Null),
        )?;
        Ok(report)
    }

    pub fn compose(
        &self,
        cwd: &Path,
        left: &str,
        right: &str,
        output: &str,
    ) -> DraftResult<ComposeResult> {
        let ws = self.open(cwd)?;
        let l = load_pack(&ws, left)?;
        let r = load_pack(&ws, right)?;
        let l_base = load_snapshot(&ws, &l.base_snapshot_id)?;
        let r_base = load_snapshot(&ws, &r.base_snapshot_id)?;
        if snapshot_file_fingerprint(&l_base) != snapshot_file_fingerprint(&r_base) {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "compose requires changepacks with the same base content",
            ));
        }
        let lp = load_patch(&ws, &l)?;
        let rp = load_patch(&ws, &r)?;
        let cmp = self.compare(cwd, left, right)?;
        if !cmp.compatible {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                "compose has overlapping changes",
            )
            .with_context(format!("{:?}", cmp.warnings)));
        }
        let mut files = lp.files.clone();
        files.extend(rp.files.clone());
        files.sort_by(|a, b| a.path.cmp(&b.path).then(a.old_path.cmp(&b.old_path)));
        let mut patch = PatchSet {
            schema_version: SCHEMA_VERSION,
            id: PatchSetId::generate(),
            base_snapshot_id: l.base_snapshot_id.clone(),
            result_snapshot_id: r.result_snapshot_id.clone(),
            files,
            patch_graph_hash: String::new(),
        };
        patch.patch_graph_hash = hash_json(&patch)?;
        let evidence = Evidence {
            schema_version: SCHEMA_VERSION,
            id: EvidenceId::generate(),
            changepack_id: ChangepackId::new("pending"),
            command_logs: vec![],
            files_touched: patch.files.iter().map(|f| f.path.clone()).collect(),
            generated_diff_ref: None,
            test_results: vec![],
            lint_results: vec![],
            risk_summary_ref: None,
            agent_plan_ref: None,
            agent_transcript_ref: None,
            warnings: vec!["composed from compatible changepacks".to_string()],
            created_at: now(),
        };
        let mut pack = Changepack::new(
            ws.id.clone(),
            l.task_id.clone().or_else(|| r.task_id.clone()),
            None,
            l.base_snapshot_id.clone(),
            r.result_snapshot_id.clone(),
            Some(output.to_string()),
        );
        let mut evidence = evidence;
        evidence.changepack_id = pack.id.clone();
        pack.source_pack_ids = vec![l.id.to_string(), r.id.to_string()];
        pack.patch_refs.push(patch.id.to_string());
        pack.evidence_refs.push(evidence.id.to_string());
        let pack_dir = ws.layout.pack_dir(&pack.id);
        ensure_dir(&pack_dir)?;
        write_json(&pack_dir.join("patch.json"), &patch)?;
        write_json(&pack_dir.join("evidence.json"), &evidence)?;
        save_pack_manifest(&ws, &mut pack)?;
        let receipt = Receipt::new(
            "compose",
            "completed",
            Some(pack.id.to_string()),
            serde_json::json!({
                "sources": pack.source_pack_ids,
                "files": patch.files.len(),
                "compare": cmp.id
            }),
        );
        write_receipt(&ws, &receipt)?;
        ws.events()?.append(
            "ChangepackComposed",
            Some(pack.id.to_string()),
            serde_json::json!({ "receipt_id": receipt.id.to_string() }),
        )?;
        Ok(ComposeResult {
            output_pack_id: pack.id.to_string(),
            source_packs: pack.source_pack_ids,
            receipt_id: receipt.id.to_string(),
            files: patch.files.len(),
            compatible: true,
        })
    }

    pub fn save(
        &self,
        cwd: &Path,
        pack_id: &str,
        vars: BTreeMap<String, String>,
    ) -> DraftResult<SaveReceipt> {
        let ws = self.open(cwd)?;
        let mut pack = load_pack(&ws, pack_id)?;
        let started = now();
        ws.events()?.append(
            "SaveStarted",
            Some(pack.id.to_string()),
            serde_json::json!({}),
        )?;
        let cfg = ResolvedConfig::load(&ws)?;
        let policy = read_or_default::<PolicyConfig>(&ws.layout.policy_toml());
        let patch = load_patch(&ws, &pack)?;
        if patch.files.iter().any(|f| is_draft_path(f.path.as_str())) {
            let receipt = failed_save(
                &ws,
                &pack,
                started,
                "Warning: .draft/ is included in the save candidate.",
            )?;
            ws.events()?.append(
                "SaveFailed",
                Some(pack.id.to_string()),
                serde_json::to_value(&receipt).unwrap_or(Value::Null),
            )?;
            return Err(DraftError::new(DraftErrorKind::SaveFailed, "Warning: .draft/ is included in the save candidate.\n\nDraft metadata must never be saved into an external repository or external system.\n\nSave aborted."));
        }
        if policy.save.block_if_tests_fail && pack.verification_refs.is_empty() {
            let receipt = failed_save(&ws, &pack, started, "verification is required before save")?;
            ws.events()?.append(
                "SaveFailed",
                Some(pack.id.to_string()),
                serde_json::to_value(&receipt).unwrap_or(Value::Null),
            )?;
            return Err(DraftError::new(
                DraftErrorKind::VerificationFailed,
                "verification is required before save",
            ));
        }
        if policy.save.block_if_unreviewed_high_risk
            && !matches!(pack.status, ChangepackStatus::Approved)
        {
            let receipt = failed_save(&ws, &pack, started, "approval is required before save")?;
            ws.events()?.append(
                "SaveFailed",
                Some(pack.id.to_string()),
                serde_json::to_value(&receipt).unwrap_or(Value::Null),
            )?;
            return Err(DraftError::new(
                DraftErrorKind::ReviewRequired,
                "approval is required before save",
            ));
        }
        let receipt_id = ReceiptId::generate();
        let rendered_message = render_message(&cfg, &pack, &patch, &receipt_id);
        let store = ObjectStore::new(ws.layout.clone());
        let message_ref = store.put_bytes(rendered_message.as_bytes())?;
        let mut receipt = SaveReceipt {
            schema_version: SCHEMA_VERSION,
            id: receipt_id,
            changepack_id: pack.id.clone(),
            actor: resolve_actor(&ws.layout.draft_dir),
            native_save_status: NativeSaveStatus::Saved,
            hook_status: HookStatus::NotConfigured,
            overall_status: SaveOverallStatus::Saved,
            message_ref,
            hook_results: Vec::new(),
            started_at: started,
            ended_at: now(),
            receipt_hash: String::new(),
            failure_reason: None,
        };
        if let Some(hook) = cfg.hook("save") {
            let ctx = HookContext {
                message: rendered_message.clone(),
                title: pack.name.clone().unwrap_or_else(|| pack.id.to_string()),
                description: String::new(),
                task_id: pack
                    .task_id
                    .as_ref()
                    .map(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                run_id: pack
                    .run_id
                    .as_ref()
                    .map(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                changepack_id: pack.id.to_string(),
                receipt_id: receipt.id.to_string(),
                actor_name: cfg.identity_username.clone(),
                actor_email: cfg.identity_email.clone(),
                timestamp: now().to_rfc3339(),
                verified: (!pack.verification_refs.is_empty()).to_string(),
                risk_level: "unknown".to_string(),
                files_changed: patch.files.len().to_string(),
                workspace_root: ws.root.display().to_string(),
                hook_name: "save".to_string(),
                hook_phase: hook.phase.clone(),
                vars,
            };
            match run_hook(&ws, &store, "save", &hook, &ctx) {
                Ok(result) => {
                    let failed = result.exit_code != 0;
                    receipt.hook_results.push(result);
                    if failed {
                        receipt.hook_status = HookStatus::Failed;
                        if hook.continue_on_error {
                            receipt.overall_status = SaveOverallStatus::SavedWithHookFailure;
                        } else {
                            receipt.overall_status = SaveOverallStatus::Failed;
                            receipt.failure_reason = Some("hooks.save failed".to_string());
                            receipt.ended_at = now();
                            receipt.receipt_hash = hash_json(&receipt)?;
                            write_save_receipt(&ws, &receipt)?;
                            ws.events()?.append(
                                "SaveFailed",
                                Some(pack.id.to_string()),
                                serde_json::to_value(&receipt).unwrap_or(Value::Null),
                            )?;
                            return Err(DraftError::new(
                                DraftErrorKind::SaveFailed,
                                "hooks.save failed",
                            ));
                        }
                    } else {
                        receipt.hook_status = HookStatus::Succeeded;
                    }
                }
                Err(e) => {
                    receipt.hook_status = HookStatus::Failed;
                    if hook.continue_on_error {
                        receipt.overall_status = SaveOverallStatus::SavedWithHookFailure;
                        receipt.failure_reason = Some(e.message);
                    } else {
                        receipt.overall_status = SaveOverallStatus::Failed;
                        receipt.failure_reason = Some(e.message.clone());
                        receipt.ended_at = now();
                        receipt.receipt_hash = hash_json(&receipt)?;
                        write_save_receipt(&ws, &receipt)?;
                        ws.events()?.append(
                            "SaveFailed",
                            Some(pack.id.to_string()),
                            serde_json::to_value(&receipt).unwrap_or(Value::Null),
                        )?;
                        return Err(DraftError::new(DraftErrorKind::SaveFailed, e.message));
                    }
                }
            }
        }
        receipt.ended_at = now();
        receipt.receipt_hash = hash_json(&receipt)?;
        write_save_receipt(&ws, &receipt)?;
        pack.receipt_refs.push(receipt.id.to_string());
        pack.status = pack.status.transition(ChangepackStatus::Saved)?;
        save_pack_manifest(&ws, &mut pack)?;
        ws.events()?.append(
            "SaveCompleted",
            Some(pack.id.to_string()),
            serde_json::to_value(&receipt).unwrap_or(Value::Null),
        )?;
        Ok(receipt)
    }

    pub fn rollback_plan(&self, cwd: &Path, reference: &str) -> DraftResult<RollbackPlan> {
        let ws = self.open(cwd)?;
        let snapshot = resolve_snapshot_reference(&ws, reference)?;
        let current = Snapshotter::new(&ws)?.create_snapshot()?;
        let patch = diff_snapshot_values(&snapshot, &current);
        Ok(RollbackPlan {
            id: RollbackPlanId::generate(),
            rollback_snapshot_id: snapshot.id,
            affected_files: patch
                .files
                .into_iter()
                .map(|f| f.path)
                .filter(|p| !is_draft_path(p.as_str()))
                .collect(),
            destructive: true,
            warnings: vec!["rollback will overwrite affected workspace files".to_string()],
        })
    }

    pub fn rollback(&self, cwd: &Path, reference: &str, yes: bool) -> DraftResult<RollbackReceipt> {
        let ws = self.open(cwd)?;
        let plan = self.rollback_plan(cwd, reference)?;
        if plan.destructive && !yes {
            return Err(DraftError::new(
                DraftErrorKind::RiskPolicyBlocked,
                "rollback is destructive; pass --yes to apply",
            ));
        }
        ws.events()?.append(
            "RollbackCreated",
            Some(plan.id.to_string()),
            serde_json::to_value(&plan).unwrap_or(Value::Null),
        )?;
        let snap = load_snapshot(&ws, &plan.rollback_snapshot_id)?;
        restore_snapshot(&ws, &snap)?;
        let mut receipt = RollbackReceipt {
            schema_version: SCHEMA_VERSION,
            id: ReceiptId::generate(),
            rollback_plan_id: plan.id,
            actor: resolve_actor(&ws.layout.draft_dir),
            status: "completed".to_string(),
            started_at: now(),
            ended_at: now(),
            receipt_hash: String::new(),
        };
        receipt.receipt_hash = hash_json(&receipt)?;
        write_json(
            &ws.layout
                .receipts_dir()
                .join(format!("{}.json", receipt.id)),
            &receipt,
        )?;
        ws.events()?.append(
            "RollbackCompleted",
            Some(receipt.id.to_string()),
            serde_json::to_value(&receipt).unwrap_or(Value::Null),
        )?;
        Ok(receipt)
    }

    pub fn receipts(&self, cwd: &Path) -> DraftResult<Vec<Value>> {
        let ws = self.open(cwd)?;
        let mut out = Vec::new();
        for p in list_with_extension(&ws.layout.receipts_dir(), "json")? {
            out.push(serde_json::from_str(&fs::read_to_string(p)?)?);
        }
        Ok(out)
    }

    pub fn receipt_show(&self, cwd: &Path, id: &str) -> DraftResult<Value> {
        let ws = self.open(cwd)?;
        let p = ws.layout.receipts_dir().join(format!("{}.json", id));
        Ok(serde_json::from_str(&fs::read_to_string(&p).map_err(
            |e| DraftError::not_found(format!("cannot read receipt {id}: {e}")),
        )?)?)
    }

    pub fn events(&self, cwd: &Path) -> DraftResult<Vec<EventEnvelope>> {
        self.open(cwd)?.events()?.read_all()
    }

    pub fn verify_events(&self, cwd: &Path) -> DraftResult<HashChainStatus> {
        self.open(cwd)?.events()?.verify_chain()
    }

    pub fn replay_events(&self, cwd: &Path) -> DraftResult<EventReplayReport> {
        let ws = self.open(cwd)?;
        let events = ws.events()?.read_all()?;
        let mut by_type = BTreeMap::new();
        for event in &events {
            *by_type.entry(event.event_type.clone()).or_insert(0usize) += 1;
        }
        let chain = ws.events()?.verify_chain()?;
        Ok(EventReplayReport {
            workspace_id: ws.id.to_string(),
            events: events.len(),
            by_type,
            chain_ok: chain.ok,
            error: chain.error,
        })
    }

    pub fn index_rebuild(&self, cwd: &Path) -> DraftResult<IndexReport> {
        let ws = self.open(cwd)?;
        rebuild_index(&ws)
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub root: PathBuf,
    pub layout: DraftLayout,
}

impl Workspace {
    fn events(&self) -> DraftResult<EventStore> {
        EventStore::new(self.layout.clone(), self.id.clone())
    }
}

#[derive(Debug, Clone)]
pub struct DraftLayout {
    pub draft_dir: PathBuf,
}

impl DraftLayout {
    pub fn for_root(root: &Path) -> Self {
        Self {
            draft_dir: root.join(DRAFT_DIR),
        }
    }
    pub fn create_all(&self) -> DraftResult<()> {
        for dir in [
            self.draft_dir.clone(),
            self.objects_dir(),
            self.events_dir(),
            self.snapshots_dir(),
            self.tasks_dir(),
            self.runs_dir(),
            self.changepacks_dir(),
            self.receipts_dir(),
            self.indexes_dir(),
            self.locks_dir(),
            self.tmp_dir(),
        ] {
            ensure_dir(&dir)?;
        }
        Ok(())
    }
    pub fn root(&self) -> PathBuf {
        self.draft_dir
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf()
    }
    pub fn config_toml(&self) -> PathBuf {
        self.draft_dir.join("config.toml")
    }
    pub fn ignore_file(&self) -> PathBuf {
        self.draft_dir.join(".ignore")
    }
    pub fn verify_toml(&self) -> PathBuf {
        self.draft_dir.join("verify.toml")
    }
    pub fn policy_toml(&self) -> PathBuf {
        self.draft_dir.join("policy.toml")
    }
    pub fn workspace_json(&self) -> PathBuf {
        self.draft_dir.join("workspace.json")
    }
    pub fn objects_dir(&self) -> PathBuf {
        self.draft_dir.join("objects/sha256")
    }
    pub fn events_dir(&self) -> PathBuf {
        self.draft_dir.join("events")
    }
    pub fn events_file(&self) -> PathBuf {
        self.events_dir().join("events.jsonl")
    }
    pub fn snapshots_dir(&self) -> PathBuf {
        self.draft_dir.join("snapshots")
    }
    pub fn tasks_dir(&self) -> PathBuf {
        self.draft_dir.join("tasks")
    }
    pub fn runs_dir(&self) -> PathBuf {
        self.draft_dir.join("runs")
    }
    pub fn changepacks_dir(&self) -> PathBuf {
        self.draft_dir.join("changepacks")
    }
    pub fn receipts_dir(&self) -> PathBuf {
        self.draft_dir.join("receipts")
    }
    pub fn indexes_dir(&self) -> PathBuf {
        self.draft_dir.join("indexes")
    }
    pub fn index_file(&self) -> PathBuf {
        self.indexes_dir().join("draft.sqlite")
    }
    pub fn locks_dir(&self) -> PathBuf {
        self.draft_dir.join("locks")
    }
    pub fn tmp_dir(&self) -> PathBuf {
        self.draft_dir.join("tmp")
    }
    pub fn pack_dir(&self, id: &ChangepackId) -> PathBuf {
        self.changepacks_dir().join(id.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMetadata {
    pub schema_version: u32,
    pub id: WorkspaceId,
    pub draft_version: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitReport {
    pub workspace_id: String,
    pub root: String,
    pub created: bool,
    pub draft_dir: String,
}

const DEFAULT_IGNORE: &str = "# Draft private metadata is always excluded.\n.draft/\n";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftConfig {
    pub identity: IdentityConfig,
    #[serde(default)]
    pub save: SaveConfig,
    #[serde(default)]
    pub hooks: HooksConfig,
    pub verification: VerificationConfig,
    pub policy: PolicyConfigSection,
}

impl Default for DraftConfig {
    fn default() -> Self {
        Self {
            identity: IdentityConfig::default(),
            save: SaveConfig::default(),
            hooks: HooksConfig::default(),
            verification: VerificationConfig {
                default_profile: "standard".to_string(),
            },
            policy: PolicyConfigSection {
                require_verification: true,
                require_approval: true,
                require_human_approval_for_high_risk: true,
                block_if_tests_fail: true,
            },
        }
    }
}

impl DraftConfig {
    fn set(&mut self, key: &str, value: &str) -> DraftResult<()> {
        match key {
            "identity.username" => self.identity.username = value.to_string(),
            "identity.email" => self.identity.email = value.to_string(),
            "save.message_template" => self.save.message_template = value.to_string(),
            "hooks.save" => self.hooks.save = Some(HookConfig::Raw(value.to_string())),
            "hooks.verify" => self.hooks.verify = Some(HookConfig::Raw(value.to_string())),
            _ => {
                return Err(DraftError::invalid_config(format!(
                    "unsupported config key '{key}'"
                )))
            }
        }
        Ok(())
    }
    fn unset(&mut self, key: &str) -> DraftResult<()> {
        self.set(key, "")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IdentityConfig {
    pub username: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveConfig {
    pub message_template: String,
}

impl Default for SaveConfig {
    fn default() -> Self {
        Self {
            message_template: "{{title}}\n\n{{description}}\n\nDraft-Task: {{task_id}}\nDraft-Run: {{run_id}}\nDraft-Changepack: {{changepack_id}}\nDraft-Verified: {{verified}}\nDraft-Risk: {{risk_level}}\nDraft-Receipt: {{receipt_id}}\nDraft-Actor: {{actor_name}} <{{actor_email}}>".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HooksConfig {
    pub save: Option<HookConfig>,
    pub verify: Option<HookConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HookConfig {
    Raw(String),
    Entry(HookEntry),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEntry {
    pub command: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_hook_phase")]
    pub phase: String,
    #[serde(default = "default_hook_shell")]
    pub shell: String,
    #[serde(default = "default_hook_cwd")]
    pub cwd: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub continue_on_error: bool,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl HookConfig {
    fn entry(&self) -> Option<HookEntry> {
        match self {
            HookConfig::Raw(command) => {
                if command.trim().is_empty() {
                    None
                } else {
                    Some(HookEntry {
                        command: command.clone(),
                        enabled: true,
                        phase: default_hook_phase(),
                        shell: default_hook_shell(),
                        cwd: default_hook_cwd(),
                        timeout_ms: None,
                        continue_on_error: false,
                        env: BTreeMap::new(),
                    })
                }
            }
            HookConfig::Entry(entry) if entry.enabled && !entry.command.trim().is_empty() => {
                Some(entry.clone())
            }
            HookConfig::Entry(_) => None,
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_hook_phase() -> String {
    "after_success".to_string()
}
fn default_hook_shell() -> String {
    "default".to_string()
}
fn default_hook_cwd() -> String {
    "workspace".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationConfig {
    pub default_profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfigSection {
    pub require_verification: bool,
    pub require_approval: bool,
    pub require_human_approval_for_high_risk: bool,
    pub block_if_tests_fail: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigReport {
    pub entries: BTreeMap<String, String>,
}

impl ConfigReport {
    fn single(key: &str, value: &str) -> Self {
        let mut entries = BTreeMap::new();
        entries.insert(key.to_string(), value.to_string());
        Self { entries }
    }
}

fn hook_config_command(hook: &HookConfig) -> String {
    match hook {
        HookConfig::Raw(command) => command.clone(),
        HookConfig::Entry(entry) => entry.command.clone(),
    }
}

#[derive(Debug, Clone)]
struct ResolvedConfig {
    identity_username: String,
    identity_email: String,
    save_message_template: String,
    hooks: HooksConfig,
}

impl ResolvedConfig {
    fn load(ws: &Workspace) -> DraftResult<Self> {
        let mut cfg = DraftConfig::default();
        if let Some(home) = home_dir() {
            let global = home.join(".draft/config.toml");
            if global.exists() {
                cfg = merge_config(cfg, read_toml(&global)?);
            }
        }
        if ws.layout.config_toml().exists() {
            cfg = merge_config(cfg, read_toml(&ws.layout.config_toml())?);
        }
        if let Ok(v) = std::env::var("DRAFT_IDENTITY_USERNAME") {
            cfg.identity.username = v;
        }
        if let Ok(v) = std::env::var("DRAFT_IDENTITY_EMAIL") {
            cfg.identity.email = v;
        }
        Ok(Self {
            identity_username: cfg.identity.username,
            identity_email: cfg.identity.email,
            save_message_template: cfg.save.message_template,
            hooks: cfg.hooks,
        })
    }
    fn hook(&self, name: &str) -> Option<HookEntry> {
        match name {
            "save" => self.hooks.save.as_ref().and_then(HookConfig::entry),
            "verify" => self.hooks.verify.as_ref().and_then(HookConfig::entry),
            _ => None,
        }
    }
    fn get(&self, key: &str) -> Option<String> {
        match key {
            "identity.username" => Some(self.identity_username.clone()),
            "identity.email" => Some(self.identity_email.clone()),
            "save.message_template" => Some(self.save_message_template.clone()),
            "hooks.save" => self.hooks.save.as_ref().map(hook_config_command),
            "hooks.verify" => self.hooks.verify.as_ref().map(hook_config_command),
            _ => None,
        }
    }
    fn entries(&self) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        for k in [
            "identity.username",
            "identity.email",
            "save.message_template",
            "hooks.save",
            "hooks.verify",
        ] {
            m.insert(k.to_string(), self.get(k).unwrap_or_default());
        }
        m
    }
}

fn merge_config(mut base: DraftConfig, overlay: DraftConfig) -> DraftConfig {
    if !overlay.identity.username.is_empty() {
        base.identity.username = overlay.identity.username;
    }
    if !overlay.identity.email.is_empty() {
        base.identity.email = overlay.identity.email;
    }
    if !overlay.save.message_template.is_empty() {
        base.save.message_template = overlay.save.message_template;
    }
    if overlay.hooks.save.is_some() {
        base.hooks.save = overlay.hooks.save;
    }
    if overlay.hooks.verify.is_some() {
        base.hooks.verify = overlay.hooks.verify;
    }
    base
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IgnoreReport {
    pub patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub id: EventId,
    #[serde(rename = "type")]
    pub event_type: String,
    pub time: DateTime<Utc>,
    pub actor: ActorRef,
    pub workspace_id: WorkspaceId,
    pub subject_id: Option<String>,
    pub payload: Value,
    pub prev_event_hash: Option<String>,
    pub event_hash: String,
    pub schema_version: u32,
}

#[derive(Debug, Clone)]
struct EventStore {
    layout: DraftLayout,
    workspace_id: WorkspaceId,
}

impl EventStore {
    fn new(layout: DraftLayout, workspace_id: WorkspaceId) -> DraftResult<Self> {
        ensure_dir(&layout.events_dir())?;
        if !layout.events_file().exists() {
            write_atomic(&layout.events_file(), b"")?;
        }
        Ok(Self {
            layout,
            workspace_id,
        })
    }
    fn append(
        &self,
        event_type: &str,
        subject_id: Option<String>,
        payload: Value,
    ) -> DraftResult<EventId> {
        let _guard = FileGuard::acquire(
            &self.layout.locks_dir().join("events.lock"),
            Duration::from_secs(10),
        )?;
        let prev = self.read_all()?.last().map(|e| e.event_hash.clone());
        let mut env = EventEnvelope {
            id: EventId::generate(),
            event_type: event_type.to_string(),
            time: now(),
            actor: resolve_actor(&self.layout.draft_dir),
            workspace_id: self.workspace_id.clone(),
            subject_id,
            payload,
            prev_event_hash: prev,
            event_hash: String::new(),
            schema_version: SCHEMA_VERSION,
        };
        env.event_hash = hash_json(&env)?;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.layout.events_file())?;
        writeln!(f, "{}", serde_json::to_string(&env).map_err(json_err)?)?;
        f.sync_all()?;
        Ok(env.id)
    }
    fn read_all(&self) -> DraftResult<Vec<EventEnvelope>> {
        let content = fs::read_to_string(self.layout.events_file()).unwrap_or_default();
        let mut out = Vec::new();
        for (idx, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            out.push(serde_json::from_str(line).map_err(|e| {
                DraftError::storage(format!("event log parse failed at line {}: {e}", idx + 1))
            })?);
        }
        Ok(out)
    }
    fn verify_chain(&self) -> DraftResult<HashChainStatus> {
        let events = self.read_all()?;
        let mut prev = None;
        for e in &events {
            if e.prev_event_hash != prev {
                return Ok(HashChainStatus {
                    ok: false,
                    events: events.len(),
                    error: Some(format!("broken prev hash at {}", e.id)),
                });
            }
            let mut clone = e.clone();
            clone.event_hash.clear();
            let hash = hash_json(&clone)?;
            if hash != e.event_hash {
                return Ok(HashChainStatus {
                    ok: false,
                    events: events.len(),
                    error: Some(format!("broken event hash at {}", e.id)),
                });
            }
            prev = Some(e.event_hash.clone());
        }
        Ok(HashChainStatus {
            ok: true,
            events: events.len(),
            error: None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HashChainStatus {
    pub ok: bool,
    pub events: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventReplayReport {
    pub workspace_id: String,
    pub events: usize,
    pub by_type: BTreeMap<String, usize>,
    pub chain_ok: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
struct ObjectStore {
    layout: DraftLayout,
}

impl ObjectStore {
    fn new(layout: DraftLayout) -> Self {
        Self { layout }
    }
    fn put_bytes(&self, data: &[u8]) -> DraftResult<String> {
        let hash = sha256_hex(data);
        let (a, rest) = hash.split_at(2);
        let path = self.layout.objects_dir().join(a).join(rest);
        if !path.exists() {
            write_atomic(&path, data)?;
        }
        Ok(format!("sha256:{hash}"))
    }
    fn get_bytes(&self, object_ref: &str) -> DraftResult<Vec<u8>> {
        let h = object_ref.strip_prefix("sha256:").unwrap_or(object_ref);
        let (a, rest) = h.split_at(2);
        let mut data = Vec::new();
        fs::File::open(self.layout.objects_dir().join(a).join(rest))?.read_to_end(&mut data)?;
        Ok(data)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceStatus {
    pub workspace_id: WorkspaceId,
    pub root_path: String,
    pub scanned_at: DateTime<Utc>,
    pub changes: Vec<FileChange>,
    pub ignored_count: usize,
    pub has_draft_dir_violation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: WorkspacePath,
    pub change_kind: FileChangeKind,
    pub file_kind: FileKind,
    pub old_hash: Option<String>,
    pub new_hash: Option<String>,
    pub size_bytes: Option<u64>,
    pub executable: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed { from: WorkspacePath },
    TypeChanged,
    PermissionChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Text,
    Binary,
    Symlink,
    Directory,
    Unknown,
}

struct Scanner<'a> {
    ws: &'a Workspace,
    ignore: IgnoreMatcher,
}

impl<'a> Scanner<'a> {
    fn new(ws: &'a Workspace) -> DraftResult<Self> {
        Ok(Self {
            ws,
            ignore: IgnoreMatcher::load(&ws.layout.ignore_file())?,
        })
    }
    fn status(&self) -> DraftResult<WorkspaceStatus> {
        let previous = latest_snapshot(self.ws)?;
        let current = self.current_manifest()?;
        let previous_map: BTreeMap<_, _> = previous
            .as_ref()
            .map(|s| {
                s.files
                    .iter()
                    .map(|f| (f.path.clone(), f.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let mut changes = diff_manifests(&previous_map, &current);
        detect_renames(&mut changes);
        Ok(WorkspaceStatus {
            workspace_id: self.ws.id.clone(),
            root_path: self.ws.root.display().to_string(),
            scanned_at: now(),
            ignored_count: self.ignore.ignored_count,
            has_draft_dir_violation: false,
            changes,
        })
    }
    fn current_manifest(&self) -> DraftResult<BTreeMap<WorkspacePath, FileManifestEntry>> {
        let mut out = BTreeMap::new();
        let store = ObjectStore::new(self.ws.layout.clone());
        walk_dir(&self.ws.root, &mut |path| {
            let rel = rel_path(&self.ws.root, path)?;
            if self.ignore.is_ignored(rel.as_str()) {
                return Ok(());
            }
            if path.is_dir() {
                return Ok(());
            }
            let meta = fs::symlink_metadata(path)?;
            let kind = file_kind(path, &meta)?;
            let (hash, size) = if matches!(kind, FileKind::Directory) {
                (None, 0)
            } else if matches!(kind, FileKind::Symlink) {
                let target = fs::read_link(path)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                (
                    Some(store.put_bytes(target.as_bytes())?),
                    target.len() as u64,
                )
            } else {
                let data = fs::read(path)?;
                (Some(store.put_bytes(&data)?), data.len() as u64)
            };
            out.insert(
                rel.clone(),
                FileManifestEntry {
                    path: rel,
                    file_kind: kind,
                    content_hash: hash,
                    size_bytes: size,
                    modified_time: meta.modified().ok().map(DateTime::<Utc>::from),
                    executable: executable(&meta),
                },
            );
            Ok(())
        })?;
        Ok(out)
    }
}

#[derive(Debug, Clone)]
struct IgnoreMatcher {
    patterns: Vec<String>,
    ignored_count: usize,
}

impl IgnoreMatcher {
    fn load(path: &Path) -> DraftResult<Self> {
        Ok(Self {
            patterns: read_ignore_lines(path)?,
            ignored_count: 0,
        })
    }
    fn is_ignored(&self, path: &str) -> bool {
        if is_draft_path(path) {
            return true;
        }
        let mut ignored = false;
        for p in &self.patterns {
            let neg = p.starts_with('!');
            let pat = p.trim_start_matches('!');
            let matched = pattern_match(pat, path);
            if matched {
                ignored = !neg;
            }
        }
        ignored
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub id: SnapshotId,
    pub workspace_id: WorkspaceId,
    pub manifest_hash: String,
    pub files: Vec<FileManifestEntry>,
    pub content_object_refs: Vec<String>,
    pub ignored_patterns_hash: String,
    pub created_at: DateTime<Utc>,
    pub created_by: ActorRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileManifestEntry {
    pub path: WorkspacePath,
    pub file_kind: FileKind,
    pub content_hash: Option<String>,
    pub size_bytes: u64,
    pub modified_time: Option<DateTime<Utc>>,
    pub executable: Option<bool>,
}

struct Snapshotter<'a> {
    ws: &'a Workspace,
}

impl<'a> Snapshotter<'a> {
    fn new(ws: &'a Workspace) -> DraftResult<Self> {
        Ok(Self { ws })
    }
    fn create_snapshot(&self) -> DraftResult<Snapshot> {
        let scanner = Scanner::new(self.ws)?;
        let mut files: Vec<_> = scanner.current_manifest()?.into_values().collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let content_object_refs = files
            .iter()
            .filter_map(|f| f.content_hash.clone())
            .collect();
        let ignored_patterns_hash = sha256_hex(
            read_ignore_lines(&self.ws.layout.ignore_file())?
                .join("\n")
                .as_bytes(),
        );
        let mut snapshot = Snapshot {
            schema_version: SCHEMA_VERSION,
            id: SnapshotId::generate(),
            workspace_id: self.ws.id.clone(),
            manifest_hash: String::new(),
            files,
            content_object_refs,
            ignored_patterns_hash,
            created_at: now(),
            created_by: resolve_actor(&self.ws.layout.draft_dir),
        };
        snapshot.manifest_hash = hash_json(&snapshot)?;
        write_json(
            &self
                .ws
                .layout
                .snapshots_dir()
                .join(format!("{}.json", snapshot.id)),
            &snapshot,
        )?;
        Ok(snapshot)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointReport {
    pub snapshot_id: String,
    pub receipt_id: String,
    pub files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub schema_version: u32,
    pub id: TaskId,
    pub title: String,
    pub description: Option<String>,
    pub created_by: ActorRef,
    pub risk_profile: Option<String>,
    pub linked_issue: Option<String>,
    pub created_at: DateTime<Utc>,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Open,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub schema_version: u32,
    pub id: RunId,
    pub task_id: TaskId,
    pub workspace_id: WorkspaceId,
    pub base_snapshot_id: SnapshotId,
    pub actor_kind: ActorKind,
    pub actor_name: String,
    pub command: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub status: RunStatus,
    pub stdout_ref: Option<String>,
    pub stderr_ref: Option<String>,
    pub exit_code: Option<i32>,
    pub result_snapshot_id: Option<SnapshotId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Started,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Changepack {
    pub schema_version: u32,
    pub id: ChangepackId,
    pub name: Option<String>,
    pub task_id: Option<TaskId>,
    pub run_id: Option<RunId>,
    pub workspace_id: WorkspaceId,
    pub base_snapshot_id: SnapshotId,
    pub result_snapshot_id: SnapshotId,
    pub patch_refs: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub verification_refs: Vec<String>,
    pub review_refs: Vec<String>,
    pub decision_refs: Vec<String>,
    pub receipt_refs: Vec<String>,
    pub source_pack_ids: Vec<String>,
    pub status: ChangepackStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub manifest_hash: String,
}

impl Changepack {
    fn new(
        workspace_id: WorkspaceId,
        task_id: Option<TaskId>,
        run_id: Option<RunId>,
        base_snapshot_id: SnapshotId,
        result_snapshot_id: SnapshotId,
        name: Option<String>,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: ChangepackId::generate(),
            name,
            task_id,
            run_id,
            workspace_id,
            base_snapshot_id,
            result_snapshot_id,
            patch_refs: vec![],
            evidence_refs: vec![],
            verification_refs: vec![],
            review_refs: vec![],
            decision_refs: vec![],
            receipt_refs: vec![],
            source_pack_ids: vec![],
            status: ChangepackStatus::Draft,
            created_at: now(),
            updated_at: now(),
            manifest_hash: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangepackStatus {
    Draft,
    Verified,
    Reviewed,
    Approved,
    Saved,
    Rejected,
    RolledBack,
}

impl ChangepackStatus {
    fn transition(self, next: ChangepackStatus) -> DraftResult<ChangepackStatus> {
        use ChangepackStatus::*;
        let ok = matches!(
            (self, next),
            (Draft, Verified)
                | (Draft, Reviewed)
                | (Draft, Rejected)
                | (Verified, Reviewed)
                | (Verified, Approved)
                | (Reviewed, Approved)
                | (Reviewed, Rejected)
                | (Approved, Saved)
                | (Saved, RolledBack)
                | (Approved, Approved)
                | (Saved, Saved)
        );
        if ok {
            Ok(next)
        } else {
            Err(DraftError::new(
                DraftErrorKind::InvalidConfig,
                format!(
                    "invalid changepack transition from {:?} to {:?}",
                    self, next
                ),
            ))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchSet {
    pub schema_version: u32,
    pub id: PatchSetId,
    pub base_snapshot_id: SnapshotId,
    pub result_snapshot_id: SnapshotId,
    pub files: Vec<FilePatch>,
    pub patch_graph_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilePatch {
    pub path: WorkspacePath,
    pub old_path: Option<WorkspacePath>,
    pub change_kind: FileChangeKind,
    #[serde(default)]
    pub hunks: Vec<PatchHunk>,
    pub binary: bool,
    pub old_hash: Option<String>,
    pub new_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchHunk {
    #[serde(default)]
    pub id: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub content_ref: String,
    #[serde(default)]
    pub old_content_hash: Option<String>,
    #[serde(default)]
    pub new_content_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HunkOverlap {
    pub path: WorkspacePath,
    pub left_hunk_id: String,
    pub right_hunk_id: String,
    pub old_start: u32,
    pub old_end: u32,
    pub new_start: u32,
    pub new_end: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub schema_version: u32,
    pub id: EvidenceId,
    pub changepack_id: ChangepackId,
    pub command_logs: Vec<String>,
    pub files_touched: Vec<WorkspacePath>,
    pub generated_diff_ref: Option<String>,
    pub test_results: Vec<String>,
    pub lint_results: Vec<String>,
    pub risk_summary_ref: Option<String>,
    pub agent_plan_ref: Option<String>,
    pub agent_transcript_ref: Option<String>,
    pub warnings: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackReport {
    pub pack: Changepack,
    pub patch: PatchSet,
    pub evidence: Option<Evidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VerifyFile {
    pub checks: Vec<VerificationCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationCheck {
    pub name: String,
    pub command: String,
    pub risk: RiskLevel,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub check_name: String,
    pub command_hash: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub exit_code: i32,
    pub stdout_ref: String,
    pub stderr_ref: String,
    pub status: VerificationStatus,
}

impl VerificationResult {
    fn skipped() -> Self {
        Self {
            check_name: "no enabled checks".to_string(),
            command_hash: sha256_hex(b"skipped"),
            started_at: now(),
            ended_at: now(),
            duration_ms: 0,
            exit_code: 0,
            stdout_ref: "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                .to_string(),
            stderr_ref: "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                .to_string(),
            status: VerificationStatus::Skipped,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Passed,
    Failed,
    Skipped,
    TimedOut,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationReport {
    pub changepack_id: String,
    pub receipt_id: String,
    pub results: Vec<VerificationResult>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl RiskLevel {
    pub fn label(self) -> &'static str {
        match self {
            RiskLevel::Low => "low",
            RiskLevel::Medium => "medium",
            RiskLevel::High => "high",
            RiskLevel::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskSummary {
    pub changepack_id: String,
    pub level: RiskLevel,
    pub score: u32,
    pub factors: Vec<String>,
    pub files_changed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfig {
    pub approval: ApprovalPolicy,
    pub agent: AgentPolicy,
    pub save: SavePolicy,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            approval: ApprovalPolicy {
                low_risk_requires: 0,
                medium_risk_requires: 1,
                high_risk_requires: 1,
            },
            agent: AgentPolicy {
                allow_network: false,
                allow_secrets: false,
                require_isolated_workspace: true,
            },
            save: SavePolicy {
                block_if_tests_fail: true,
                block_if_unreviewed_high_risk: true,
                block_if_draft_dir_in_candidate: true,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalPolicy {
    pub low_risk_requires: u32,
    pub medium_risk_requires: u32,
    pub high_risk_requires: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPolicy {
    pub allow_network: bool,
    pub allow_secrets: bool,
    pub require_isolated_workspace: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavePolicy {
    pub block_if_tests_fail: bool,
    pub block_if_unreviewed_high_risk: bool,
    pub block_if_draft_dir_in_candidate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ReviewFile {
    comments: Vec<ReviewComment>,
    decisions: Vec<Decision>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewComment {
    pub id: ReviewCommentId,
    pub changepack_id: ChangepackId,
    pub path: Option<WorkspacePath>,
    pub hunk_id: Option<String>,
    pub actor: ActorRef,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Approve,
    Reject,
    NeedsChanges,
    AcceptFile,
    RejectFile,
    AcceptCandidate,
}

impl DecisionKind {
    pub fn label(self) -> &'static str {
        match self {
            DecisionKind::Approve => "approve",
            DecisionKind::Reject => "reject",
            DecisionKind::NeedsChanges => "needs_changes",
            DecisionKind::AcceptFile => "accept_file",
            DecisionKind::RejectFile => "reject_file",
            DecisionKind::AcceptCandidate => "accept_candidate",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: DecisionId,
    pub changepack_id: ChangepackId,
    pub actor: ActorRef,
    pub kind: DecisionKind,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewReport {
    pub changepack_id: String,
    pub comments: usize,
    pub decisions: usize,
    pub status: ChangepackStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompareReport {
    pub id: String,
    pub left_pack: String,
    pub right_pack: String,
    pub overlapping_files: Vec<WorkspacePath>,
    #[serde(default)]
    pub overlapping_hunks: Vec<HunkOverlap>,
    pub unique_left_files: Vec<WorkspacePath>,
    pub unique_right_files: Vec<WorkspacePath>,
    #[serde(default)]
    pub compatible: bool,
    pub warnings: Vec<String>,
    pub recommendation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeResult {
    pub output_pack_id: String,
    pub source_packs: Vec<String>,
    pub receipt_id: String,
    #[serde(default)]
    pub files: usize,
    #[serde(default)]
    pub compatible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveReceipt {
    pub schema_version: u32,
    pub id: ReceiptId,
    pub changepack_id: ChangepackId,
    pub actor: ActorRef,
    pub native_save_status: NativeSaveStatus,
    pub hook_status: HookStatus,
    pub overall_status: SaveOverallStatus,
    pub message_ref: String,
    pub hook_results: Vec<HookResult>,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub receipt_hash: String,
    pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeSaveStatus {
    Saved,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookStatus {
    NotConfigured,
    Skipped,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveOverallStatus {
    Saved,
    Failed,
    SavedWithHookFailure,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResult {
    pub hook_name: String,
    pub hook_phase: String,
    pub shell: String,
    pub working_dir: String,
    pub command_hash: String,
    pub exit_code: i32,
    pub stdout_ref: String,
    pub stderr_ref: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub env_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackPlan {
    pub id: RollbackPlanId,
    pub rollback_snapshot_id: SnapshotId,
    pub affected_files: Vec<WorkspacePath>,
    pub destructive: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackReceipt {
    pub schema_version: u32,
    pub id: ReceiptId,
    pub rollback_plan_id: RollbackPlanId,
    pub actor: ActorRef,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub receipt_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexReport {
    pub path: String,
    pub events: usize,
    pub tasks: usize,
    pub runs: usize,
    pub changepacks: usize,
    pub receipts: usize,
    pub snapshots: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Receipt {
    schema_version: u32,
    id: ReceiptId,
    kind: String,
    status: String,
    subject_id: Option<String>,
    payload: Value,
    created_at: DateTime<Utc>,
    receipt_hash: String,
}

impl Receipt {
    fn new(kind: &str, status: &str, subject_id: Option<String>, payload: Value) -> Self {
        let mut r = Self {
            schema_version: SCHEMA_VERSION,
            id: ReceiptId::generate(),
            kind: kind.to_string(),
            status: status.to_string(),
            subject_id,
            payload,
            created_at: now(),
            receipt_hash: String::new(),
        };
        r.receipt_hash = hash_json(&r).unwrap_or_default();
        r
    }
}

fn find_workspace_root(cwd: &Path) -> Option<PathBuf> {
    let mut cur = cwd
        .canonicalize()
        .ok()
        .or_else(|| Some(cwd.to_path_buf()))?;
    loop {
        if cur.join(DRAFT_DIR).is_dir() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

fn reject_remote_key(key: &str) -> DraftResult<()> {
    if key.starts_with("target.") {
        return Err(DraftError::invalid_config(
            "retired external-action config keys are not supported in Draft v0.3.0; use hooks.*",
        ));
    }
    Ok(())
}

fn validate_config_key(key: &str) -> DraftResult<()> {
    if key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_')
        && key.contains('.')
    {
        Ok(())
    } else {
        Err(DraftError::invalid_config(
            "config keys must be lowercase dotted keys",
        ))
    }
}

fn read_or_default<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    if path.exists() {
        read_toml(path).unwrap_or_default()
    } else {
        T::default()
    }
}

fn read_ignore_lines(path: &Path) -> DraftResult<Vec<String>> {
    let content = fs::read_to_string(path).unwrap_or_default();
    Ok(content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(ToString::to_string)
        .collect())
}

fn is_draft_path(path: &str) -> bool {
    let p = path.replace('\\', "/");
    p == ".draft" || p.starts_with(".draft/")
}

fn pattern_match(pattern: &str, path: &str) -> bool {
    if pattern == ".draft/" {
        return is_draft_path(path);
    }
    if let Some(dir) = pattern.strip_suffix('/') {
        return path == dir || path.starts_with(&format!("{dir}/"));
    }
    if let Some(ext) = pattern.strip_prefix("*.") {
        return path
            .rsplit('/')
            .next()
            .unwrap_or(path)
            .ends_with(&format!(".{ext}"));
    }
    if pattern.contains('*') {
        let parts: Vec<_> = pattern.split('*').collect();
        let mut rem = path;
        for part in parts {
            if part.is_empty() {
                continue;
            }
            if let Some(idx) = rem.find(part) {
                rem = &rem[idx + part.len()..];
            } else {
                return false;
            }
        }
        return true;
    }
    path == pattern || path.starts_with(&format!("{pattern}/"))
}

fn rel_path(root: &Path, path: &Path) -> DraftResult<WorkspacePath> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| DraftError::storage("path escaped workspace root"))?;
    Ok(WorkspacePath::from_relative(rel))
}

fn walk_dir<F: FnMut(&Path) -> DraftResult<()>>(root: &Path, f: &mut F) -> DraftResult<()> {
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        let rel = rel_path(root, &path)?;
        if is_draft_path(rel.as_str()) {
            continue;
        }
        f(&path)?;
        if path.is_dir() {
            walk_dir_inner(root, &path, f)?;
        }
    }
    Ok(())
}

fn walk_dir_inner<F: FnMut(&Path) -> DraftResult<()>>(
    root: &Path,
    dir: &Path,
    f: &mut F,
) -> DraftResult<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let rel = rel_path(root, &path)?;
        if is_draft_path(rel.as_str()) {
            continue;
        }
        f(&path)?;
        if path.is_dir() {
            walk_dir_inner(root, &path, f)?;
        }
    }
    Ok(())
}

fn file_kind(path: &Path, meta: &fs::Metadata) -> DraftResult<FileKind> {
    if meta.file_type().is_symlink() {
        return Ok(FileKind::Symlink);
    }
    if meta.is_dir() {
        return Ok(FileKind::Directory);
    }
    let mut buf = [0u8; 1024];
    let n = fs::File::open(path)
        .and_then(|mut f| f.read(&mut buf))
        .unwrap_or(0);
    if buf[..n].contains(&0) {
        Ok(FileKind::Binary)
    } else {
        Ok(FileKind::Text)
    }
}

#[cfg(unix)]
fn executable(meta: &fs::Metadata) -> Option<bool> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(_meta: &fs::Metadata) -> Option<bool> {
    None
}

fn diff_manifests(
    old: &BTreeMap<WorkspacePath, FileManifestEntry>,
    new: &BTreeMap<WorkspacePath, FileManifestEntry>,
) -> Vec<FileChange> {
    let mut changes = Vec::new();
    for (path, n) in new {
        match old.get(path) {
            None => changes.push(FileChange {
                path: path.clone(),
                change_kind: FileChangeKind::Added,
                file_kind: n.file_kind.clone(),
                old_hash: None,
                new_hash: n.content_hash.clone(),
                size_bytes: Some(n.size_bytes),
                executable: n.executable,
            }),
            Some(o)
                if o.content_hash != n.content_hash
                    || o.file_kind != n.file_kind
                    || o.executable != n.executable =>
            {
                changes.push(FileChange {
                    path: path.clone(),
                    change_kind: if o.file_kind != n.file_kind {
                        FileChangeKind::TypeChanged
                    } else if o.executable != n.executable {
                        FileChangeKind::PermissionChanged
                    } else {
                        FileChangeKind::Modified
                    },
                    file_kind: n.file_kind.clone(),
                    old_hash: o.content_hash.clone(),
                    new_hash: n.content_hash.clone(),
                    size_bytes: Some(n.size_bytes),
                    executable: n.executable,
                })
            }
            _ => {}
        }
    }
    for (path, o) in old {
        if !new.contains_key(path) {
            changes.push(FileChange {
                path: path.clone(),
                change_kind: FileChangeKind::Deleted,
                file_kind: o.file_kind.clone(),
                old_hash: o.content_hash.clone(),
                new_hash: None,
                size_bytes: Some(o.size_bytes),
                executable: o.executable,
            });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    changes
}

fn detect_renames(changes: &mut [FileChange]) {
    let deleted: Vec<_> = changes
        .iter()
        .filter(|c| matches!(c.change_kind, FileChangeKind::Deleted))
        .map(|c| (c.old_hash.clone(), c.path.clone()))
        .collect();
    for c in changes
        .iter_mut()
        .filter(|c| matches!(c.change_kind, FileChangeKind::Added))
    {
        if let Some((_, from)) = deleted
            .iter()
            .find(|(h, _)| h.is_some() && h == &c.new_hash)
        {
            c.change_kind = FileChangeKind::Renamed { from: from.clone() };
        }
    }
}

fn latest_snapshot(ws: &Workspace) -> DraftResult<Option<Snapshot>> {
    let mut snaps: Vec<Snapshot> = load_json_dir(&ws.layout.snapshots_dir())?;
    snaps.sort_by_key(|a| a.created_at);
    Ok(snaps.pop())
}

fn empty_snapshot(ws: &Workspace) -> Snapshot {
    Snapshot {
        schema_version: SCHEMA_VERSION,
        id: SnapshotId::new("snap_empty"),
        workspace_id: ws.id.clone(),
        manifest_hash: sha256_hex(b"empty"),
        files: vec![],
        content_object_refs: vec![],
        ignored_patterns_hash: sha256_hex(b""),
        created_at: now(),
        created_by: ActorRef {
            id: ActorId::new("act_system"),
            kind: ActorKind::Service,
            display_name: "draft".to_string(),
        },
    }
}

fn load_snapshot(ws: &Workspace, id: &SnapshotId) -> DraftResult<Snapshot> {
    if id.as_str() == "snap_empty" {
        return Ok(empty_snapshot(ws));
    }
    read_json(&ws.layout.snapshots_dir().join(format!("{}.json", id)))
}

fn snapshot_file_fingerprint(snapshot: &Snapshot) -> String {
    let mut files = snapshot.files.clone();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let stable = files
        .into_iter()
        .map(|f| {
            serde_json::json!({
                "path": f.path,
                "file_kind": f.file_kind,
                "content_hash": f.content_hash,
                "size_bytes": f.size_bytes,
                "executable": f.executable,
            })
        })
        .collect::<Vec<_>>();
    sha256_hex(canonical_json(&Value::Array(stable)).as_bytes())
}

fn diff_snapshots(ws: &Workspace, base: &Snapshot, result: &Snapshot) -> DraftResult<PatchSet> {
    let mut patch = diff_snapshot_values(base, result);
    enrich_patch_hunks(ws, base, result, &mut patch)?;
    patch.patch_graph_hash.clear();
    patch.patch_graph_hash = hash_json(&patch)?;
    write_json(
        &ws.layout.tmp_dir().join(format!("{}.json", patch.id)),
        &patch,
    )?;
    Ok(patch)
}

fn diff_snapshot_values(base: &Snapshot, result: &Snapshot) -> PatchSet {
    let old: BTreeMap<_, _> = base
        .files
        .iter()
        .map(|f| (f.path.clone(), f.clone()))
        .collect();
    let new: BTreeMap<_, _> = result
        .files
        .iter()
        .map(|f| (f.path.clone(), f.clone()))
        .collect();
    let files = diff_manifests(&old, &new)
        .into_iter()
        .map(|c| FilePatch {
            path: c.path,
            old_path: match &c.change_kind {
                FileChangeKind::Renamed { from } => Some(from.clone()),
                _ => None,
            },
            change_kind: c.change_kind,
            hunks: vec![],
            binary: matches!(c.file_kind, FileKind::Binary),
            old_hash: c.old_hash,
            new_hash: c.new_hash,
        })
        .collect::<Vec<_>>();
    let mut patch = PatchSet {
        schema_version: SCHEMA_VERSION,
        id: PatchSetId::generate(),
        base_snapshot_id: base.id.clone(),
        result_snapshot_id: result.id.clone(),
        files,
        patch_graph_hash: String::new(),
    };
    patch.patch_graph_hash = hash_json(&patch).unwrap_or_default();
    patch
}

fn enrich_patch_hunks(
    ws: &Workspace,
    base: &Snapshot,
    result: &Snapshot,
    patch: &mut PatchSet,
) -> DraftResult<()> {
    let store = ObjectStore::new(ws.layout.clone());
    let old_by_path: BTreeMap<_, _> = base
        .files
        .iter()
        .map(|f| (f.path.clone(), f.clone()))
        .collect();
    let new_by_path: BTreeMap<_, _> = result
        .files
        .iter()
        .map(|f| (f.path.clone(), f.clone()))
        .collect();
    for file in &mut patch.files {
        if file.binary {
            continue;
        }
        let old_entry = file
            .old_path
            .as_ref()
            .and_then(|p| old_by_path.get(p))
            .or_else(|| old_by_path.get(&file.path));
        let new_entry = new_by_path.get(&file.path);
        let old_text = read_text_object(&store, old_entry.and_then(|e| e.content_hash.as_ref()))?;
        let new_text = read_text_object(&store, new_entry.and_then(|e| e.content_hash.as_ref()))?;
        if old_text.is_none() && new_text.is_none() {
            continue;
        }
        file.hunks = build_text_hunks(
            &store,
            &file.path,
            old_text.as_deref().unwrap_or(""),
            new_text.as_deref().unwrap_or(""),
        )?;
    }
    Ok(())
}

fn read_text_object(
    store: &ObjectStore,
    object_ref: Option<&String>,
) -> DraftResult<Option<String>> {
    let Some(object_ref) = object_ref else {
        return Ok(None);
    };
    let bytes = store.get_bytes(object_ref)?;
    match String::from_utf8(bytes) {
        Ok(s) => Ok(Some(s)),
        Err(_) => Ok(None),
    }
}

fn build_text_hunks(
    store: &ObjectStore,
    path: &WorkspacePath,
    old_text: &str,
    new_text: &str,
) -> DraftResult<Vec<PatchHunk>> {
    if old_text == new_text {
        return Ok(Vec::new());
    }
    let old_lines = split_lines_preserve(old_text);
    let new_lines = split_lines_preserve(new_text);
    let mut prefix = 0usize;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix + prefix < old_lines.len()
        && suffix + prefix < new_lines.len()
        && old_lines[old_lines.len() - 1 - suffix] == new_lines[new_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let old_changed = &old_lines[prefix..old_lines.len().saturating_sub(suffix)];
    let new_changed = &new_lines[prefix..new_lines.len().saturating_sub(suffix)];
    let old_start = prefix as u32 + 1;
    let new_start = prefix as u32 + 1;
    let old_joined = old_changed.concat();
    let new_joined = new_changed.concat();
    let hunk_body = format!(
        "--- {}\n+++ {}\n@@ -{},{} +{},{} @@\n{}{}",
        path,
        path,
        old_start,
        old_changed.len(),
        new_start,
        new_changed.len(),
        old_changed
            .iter()
            .map(|l| format!("-{l}"))
            .collect::<String>(),
        new_changed
            .iter()
            .map(|l| format!("+{l}"))
            .collect::<String>(),
    );
    let old_hash = if old_joined.is_empty() {
        None
    } else {
        Some(format!("sha256:{}", sha256_hex(old_joined.as_bytes())))
    };
    let new_hash = if new_joined.is_empty() {
        None
    } else {
        Some(format!("sha256:{}", sha256_hex(new_joined.as_bytes())))
    };
    let id_input = format!(
        "{}:{}:{}:{}:{}:{}",
        path,
        old_start,
        old_changed.len(),
        new_start,
        new_changed.len(),
        sha256_hex(hunk_body.as_bytes())
    );
    Ok(vec![PatchHunk {
        id: format!("hunk_{}", &sha256_hex(id_input.as_bytes())[..12]),
        old_start,
        old_lines: old_changed.len() as u32,
        new_start,
        new_lines: new_changed.len() as u32,
        content_ref: store.put_bytes(hunk_body.as_bytes())?,
        old_content_hash: old_hash,
        new_content_hash: new_hash,
    }])
}

fn split_lines_preserve(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split_inclusive('\n')
        .map(ToString::to_string)
        .collect()
}

fn file_level_conflict(left: &FilePatch, right: &FilePatch) -> bool {
    left.hunks.is_empty()
        || right.hunks.is_empty()
        || left.binary
        || right.binary
        || !matches!(
            (&left.change_kind, &right.change_kind),
            (FileChangeKind::Modified, FileChangeKind::Modified)
        )
}

fn hunk_overlaps(left: &PatchSet, right: &PatchSet) -> Vec<HunkOverlap> {
    let mut out = Vec::new();
    for lf in &left.files {
        for rf in right.files.iter().filter(|rf| rf.path == lf.path) {
            if file_level_conflict(lf, rf) {
                continue;
            }
            for lh in &lf.hunks {
                for rh in &rf.hunks {
                    if ranges_overlap(lh.old_start, lh.old_lines, rh.old_start, rh.old_lines)
                        || ranges_overlap(lh.new_start, lh.new_lines, rh.new_start, rh.new_lines)
                    {
                        out.push(HunkOverlap {
                            path: lf.path.clone(),
                            left_hunk_id: lh.id.clone(),
                            right_hunk_id: rh.id.clone(),
                            old_start: lh.old_start.min(rh.old_start),
                            old_end: range_end(lh.old_start, lh.old_lines)
                                .max(range_end(rh.old_start, rh.old_lines)),
                            new_start: lh.new_start.min(rh.new_start),
                            new_end: range_end(lh.new_start, lh.new_lines)
                                .max(range_end(rh.new_start, rh.new_lines)),
                        });
                    }
                }
            }
        }
    }
    out
}

fn ranges_overlap(a_start: u32, a_len: u32, b_start: u32, b_len: u32) -> bool {
    let a_end = range_end(a_start, a_len);
    let b_end = range_end(b_start, b_len);
    a_start <= b_end && b_start <= a_end
}

fn range_end(start: u32, len: u32) -> u32 {
    if len == 0 {
        start
    } else {
        start + len - 1
    }
}

fn load_json_dir<T: for<'de> Deserialize<'de>>(dir: &Path) -> DraftResult<Vec<T>> {
    let mut out = Vec::new();
    for p in list_with_extension(dir, "json")? {
        out.push(read_json(&p)?);
    }
    Ok(out)
}

fn load_pack(ws: &Workspace, id: &str) -> DraftResult<Changepack> {
    read_json(&ws.layout.changepacks_dir().join(id).join("manifest.json"))
}

fn save_pack_manifest(ws: &Workspace, pack: &mut Changepack) -> DraftResult<()> {
    pack.updated_at = now();
    pack.manifest_hash.clear();
    pack.manifest_hash = hash_json(pack)?;
    write_json(&ws.layout.pack_dir(&pack.id).join("manifest.json"), pack)
}

fn load_patch(ws: &Workspace, pack: &Changepack) -> DraftResult<PatchSet> {
    read_json(&ws.layout.pack_dir(&pack.id).join("patch.json"))
}

fn load_evidence(ws: &Workspace, pack: &Changepack) -> DraftResult<Evidence> {
    read_json(&ws.layout.pack_dir(&pack.id).join("evidence.json"))
}

fn load_review_file(ws: &Workspace, id: &ChangepackId) -> DraftResult<ReviewFile> {
    read_json(&ws.layout.pack_dir(id).join("review.json"))
}

fn save_review_file(ws: &Workspace, id: &ChangepackId, file: &ReviewFile) -> DraftResult<()> {
    write_json(&ws.layout.pack_dir(id).join("review.json"), file)
}

fn write_receipt(ws: &Workspace, receipt: &Receipt) -> DraftResult<()> {
    write_json(
        &ws.layout
            .receipts_dir()
            .join(format!("{}.json", receipt.id)),
        receipt,
    )
}

fn write_save_receipt(ws: &Workspace, receipt: &SaveReceipt) -> DraftResult<()> {
    write_json(
        &ws.layout
            .receipts_dir()
            .join(format!("{}.json", receipt.id)),
        receipt,
    )?;
    write_json(
        &ws.layout
            .pack_dir(&receipt.changepack_id)
            .join("receipts.json"),
        receipt,
    )
}

fn rebuild_index(ws: &Workspace) -> DraftResult<IndexReport> {
    rebuild_index_for_layout(&ws.layout)?;
    let conn = open_index(&ws.layout)?;
    conn.execute("DELETE FROM events", []).map_err(sql_err)?;
    conn.execute("DELETE FROM tasks", []).map_err(sql_err)?;
    conn.execute("DELETE FROM runs", []).map_err(sql_err)?;
    conn.execute("DELETE FROM changepacks", [])
        .map_err(sql_err)?;
    conn.execute("DELETE FROM receipts", []).map_err(sql_err)?;
    conn.execute("DELETE FROM snapshots", []).map_err(sql_err)?;

    let events = ws.events()?.read_all()?;
    for event in &events {
        conn.execute(
            "INSERT INTO events (id, event_type, subject_id, time, event_hash) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event.id.to_string(),
                event.event_type,
                event.subject_id,
                event.time.to_rfc3339(),
                event.event_hash
            ],
        )
        .map_err(sql_err)?;
    }

    let tasks: Vec<Task> = load_json_dir(&ws.layout.tasks_dir())?;
    for task in &tasks {
        conn.execute(
            "INSERT INTO tasks (id, title, status, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                task.id.to_string(),
                task.title,
                format!("{:?}", task.status),
                task.created_at.to_rfc3339()
            ],
        )
        .map_err(sql_err)?;
    }

    let runs: Vec<Run> = load_json_dir(&ws.layout.runs_dir())?;
    for run in &runs {
        conn.execute(
            "INSERT INTO runs (id, task_id, status, started_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                run.id.to_string(),
                run.task_id.to_string(),
                format!("{:?}", run.status),
                run.started_at.to_rfc3339()
            ],
        )
        .map_err(sql_err)?;
    }

    let packs = App::new().pack_list(&ws.root)?;
    for pack in &packs {
        conn.execute(
            "INSERT INTO changepacks (id, name, status, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                pack.id.to_string(),
                pack.name.clone().unwrap_or_default(),
                format!("{:?}", pack.status),
                pack.created_at.to_rfc3339(),
                pack.updated_at.to_rfc3339()
            ],
        )
        .map_err(sql_err)?;
    }

    let receipts = App::new().receipts(&ws.root)?;
    for receipt in &receipts {
        conn.execute(
            "INSERT INTO receipts (id, kind, status, subject_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                receipt.get("id").and_then(Value::as_str).unwrap_or_default(),
                receipt.get("kind").and_then(Value::as_str).unwrap_or_else(|| {
                    if receipt.get("hook_results").is_some()
                        || receipt.get("overall_status").is_some()
                    {
                        "save"
                    } else {
                        "receipt"
                    }
                }),
                receipt
                    .get("status")
                    .and_then(Value::as_str)
                    .or_else(|| receipt.get("overall_status").and_then(Value::as_str))
                    .unwrap_or_default(),
                receipt
                    .get("subject_id")
                    .and_then(Value::as_str)
                    .or_else(|| receipt.get("changepack_id").and_then(Value::as_str))
                    .unwrap_or_default(),
                receipt
                    .get("created_at")
                    .or_else(|| receipt.get("started_at"))
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ],
        )
        .map_err(sql_err)?;
    }

    let snapshots: Vec<Snapshot> = load_json_dir(&ws.layout.snapshots_dir())?;
    for snapshot in &snapshots {
        conn.execute(
            "INSERT INTO snapshots (id, manifest_hash, created_at, file_count) VALUES (?1, ?2, ?3, ?4)",
            params![
                snapshot.id.to_string(),
                snapshot.manifest_hash,
                snapshot.created_at.to_rfc3339(),
                snapshot.files.len() as i64
            ],
        )
        .map_err(sql_err)?;
    }

    Ok(IndexReport {
        path: ws.layout.index_file().display().to_string(),
        events: events.len(),
        tasks: tasks.len(),
        runs: runs.len(),
        changepacks: packs.len(),
        receipts: receipts.len(),
        snapshots: snapshots.len(),
    })
}

fn rebuild_index_for_layout(layout: &DraftLayout) -> DraftResult<()> {
    let conn = open_index(layout)?;
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS schema_info (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS events (
            id TEXT PRIMARY KEY,
            event_type TEXT NOT NULL,
            subject_id TEXT,
            time TEXT NOT NULL,
            event_hash TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS runs (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL,
            status TEXT NOT NULL,
            started_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS changepacks (
            id TEXT PRIMARY KEY,
            name TEXT,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS receipts (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            status TEXT NOT NULL,
            subject_id TEXT,
            created_at TEXT
        );
        CREATE TABLE IF NOT EXISTS snapshots (
            id TEXT PRIMARY KEY,
            manifest_hash TEXT NOT NULL,
            created_at TEXT NOT NULL,
            file_count INTEGER NOT NULL
        );
        ",
    )
    .map_err(sql_err)?;
    conn.execute(
        "INSERT OR REPLACE INTO schema_info (key, value) VALUES ('schema_version', ?1)",
        params![SCHEMA_VERSION.to_string()],
    )
    .map_err(sql_err)?;
    Ok(())
}

fn open_index(layout: &DraftLayout) -> DraftResult<Connection> {
    ensure_dir(&layout.indexes_dir())?;
    Connection::open(layout.index_file()).map_err(sql_err)
}

fn sql_err(e: rusqlite::Error) -> DraftError {
    DraftError::storage(format!("SQLite index error: {e}"))
}

fn failed_save(
    ws: &Workspace,
    pack: &Changepack,
    started: DateTime<Utc>,
    reason: &str,
) -> DraftResult<SaveReceipt> {
    let store = ObjectStore::new(ws.layout.clone());
    let mut receipt = SaveReceipt {
        schema_version: SCHEMA_VERSION,
        id: ReceiptId::generate(),
        changepack_id: pack.id.clone(),
        actor: resolve_actor(&ws.layout.draft_dir),
        native_save_status: NativeSaveStatus::Failed,
        hook_status: HookStatus::Skipped,
        overall_status: SaveOverallStatus::Failed,
        message_ref: store.put_bytes(b"")?,
        hook_results: Vec::new(),
        started_at: started,
        ended_at: now(),
        receipt_hash: String::new(),
        failure_reason: Some(reason.to_string()),
    };
    receipt.receipt_hash = hash_json(&receipt)?;
    write_save_receipt(ws, &receipt)?;
    Ok(receipt)
}

fn render_message(
    cfg: &ResolvedConfig,
    pack: &Changepack,
    patch: &PatchSet,
    receipt_id: &ReceiptId,
) -> String {
    let title = pack.name.clone().unwrap_or_else(|| pack.id.to_string());
    let mut values = BTreeMap::new();
    values.insert("message".to_string(), title.clone());
    values.insert("title".to_string(), title);
    values.insert("description".to_string(), String::new());
    values.insert(
        "task_id".to_string(),
        pack.task_id
            .as_ref()
            .map(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    );
    values.insert(
        "run_id".to_string(),
        pack.run_id
            .as_ref()
            .map(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    );
    values.insert("changepack_id".to_string(), pack.id.to_string());
    values.insert("receipt_id".to_string(), receipt_id.to_string());
    values.insert("actor_name".to_string(), cfg.identity_username.clone());
    values.insert("actor_email".to_string(), cfg.identity_email.clone());
    values.insert("timestamp".to_string(), now().to_rfc3339());
    values.insert(
        "verified".to_string(),
        (!pack.verification_refs.is_empty()).to_string(),
    );
    values.insert("risk_level".to_string(), "unknown".to_string());
    values.insert("files_changed".to_string(), patch.files.len().to_string());
    interpolate_lenient(&cfg.save_message_template, &values)
}

fn resolve_snapshot_reference(ws: &Workspace, reference: &str) -> DraftResult<Snapshot> {
    if reference.starts_with("snap_") {
        return load_snapshot(ws, &SnapshotId::new(reference));
    }
    if let Ok(pack) = load_pack(ws, reference) {
        return load_snapshot(ws, &pack.base_snapshot_id);
    }
    let receipt_path = ws.layout.receipts_dir().join(format!("{reference}.json"));
    if receipt_path.exists() {
        let value: Value = read_json(&receipt_path)?;
        if let Some(snapshot_id) = value.get("subject_id").and_then(Value::as_str) {
            if snapshot_id.starts_with("snap_") {
                return load_snapshot(ws, &SnapshotId::new(snapshot_id));
            }
        }
    }
    Err(DraftError::not_found(format!(
        "unknown rollback reference '{reference}'"
    )))
}

fn restore_snapshot(ws: &Workspace, snap: &Snapshot) -> DraftResult<()> {
    let store = ObjectStore::new(ws.layout.clone());
    let desired: BTreeSet<_> = snap.files.iter().map(|f| f.path.clone()).collect();
    let scanner = Scanner::new(ws)?;
    for path in scanner.current_manifest()?.keys() {
        if !desired.contains(path) && !is_draft_path(path.as_str()) {
            let fs_path = safe_workspace_dest(&ws.root, path)?;
            if fs_path.is_file() || fs_path.is_symlink() {
                fs::remove_file(fs_path)?;
            }
        }
    }
    for f in &snap.files {
        if is_draft_path(f.path.as_str()) {
            continue;
        }
        let dest = safe_workspace_dest(&ws.root, &f.path)?;
        if let Some(parent) = dest.parent() {
            ensure_dir(parent)?;
        }
        if let Some(hash) = &f.content_hash {
            let bytes = store.get_bytes(hash)?;
            write_atomic(&dest, &bytes)?;
        }
    }
    Ok(())
}

fn safe_workspace_dest(root: &Path, rel: &WorkspacePath) -> DraftResult<PathBuf> {
    if rel.as_str().is_empty()
        || rel.as_str().starts_with('/')
        || rel.as_str().contains('\0')
        || rel.as_str().split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || (cfg!(windows) && (part.contains(':') || part.contains('\\')))
        })
        || is_draft_path(rel.as_str())
    {
        return Err(DraftError::storage(format!(
            "unsafe workspace path '{}'",
            rel.as_str()
        )));
    }
    let root_canon = root
        .canonicalize()
        .map_err(|e| DraftError::storage(format!("cannot canonicalize workspace root: {e}")))?;
    let dest = root.join(rel.as_str());
    if let Some(parent) = dest.parent() {
        if parent.exists() {
            let parent_canon = parent.canonicalize().map_err(|e| {
                DraftError::storage(format!(
                    "cannot canonicalize rollback parent {}: {e}",
                    parent.display()
                ))
            })?;
            if !parent_canon.starts_with(&root_canon) {
                return Err(DraftError::storage(format!(
                    "rollback path escapes workspace: '{}'",
                    rel.as_str()
                )));
            }
        }
    }
    Ok(dest)
}

#[derive(Debug)]
struct HookContext {
    message: String,
    title: String,
    description: String,
    task_id: String,
    run_id: String,
    changepack_id: String,
    receipt_id: String,
    actor_name: String,
    actor_email: String,
    timestamp: String,
    verified: String,
    risk_level: String,
    files_changed: String,
    workspace_root: String,
    hook_name: String,
    hook_phase: String,
    vars: BTreeMap<String, String>,
}

#[derive(Debug)]
struct HookFailure {
    message: String,
}

fn run_hook(
    ws: &Workspace,
    store: &ObjectStore,
    hook_name: &str,
    hook: &HookEntry,
    ctx: &HookContext,
) -> Result<HookResult, HookFailure> {
    let mut values = hook_values(ctx);
    for (k, v) in &ctx.vars {
        values.insert(k.clone(), v.clone());
    }
    let command = interpolate_strict(&hook.command, &values)?;
    let shell_name = default_shell();
    let cwd = match hook.cwd.as_str() {
        "workspace" | "" => ws.root.clone(),
        other => ws.root.join(other),
    };
    let mut env = hook_env(ctx);
    for (k, v) in &hook.env {
        env.insert(k.clone(), v.clone());
    }
    let mut env_keys: Vec<String> = env.keys().cloned().collect();
    env_keys.sort();
    let hash = command_hash(&shell_name, &cwd, &command, &ctx.message);
    let started_at = now();
    let out = shell_with_env(&command, &cwd, &env);
    let ended_at = now();
    let (exit_code, stdout, stderr) = match out {
        Ok(o) => (o.status.code().unwrap_or(-1), o.stdout, o.stderr),
        Err(e) => (-1, Vec::new(), e.to_string().into_bytes()),
    };
    let stdout_ref = store
        .put_bytes(&stdout)
        .map_err(|e| HookFailure { message: e.message })?;
    let stderr_ref = store
        .put_bytes(&stderr)
        .map_err(|e| HookFailure { message: e.message })?;
    Ok(HookResult {
        hook_name: hook_name.to_string(),
        hook_phase: hook.phase.clone(),
        shell: shell_name,
        working_dir: cwd.display().to_string(),
        command_hash: hash,
        exit_code,
        stdout_ref,
        stderr_ref,
        started_at,
        ended_at,
        env_keys,
    })
}

fn hook_values(ctx: &HookContext) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("message".to_string(), ctx.message.clone()),
        ("title".to_string(), ctx.title.clone()),
        ("description".to_string(), ctx.description.clone()),
        ("task_id".to_string(), ctx.task_id.clone()),
        ("run_id".to_string(), ctx.run_id.clone()),
        ("changepack_id".to_string(), ctx.changepack_id.clone()),
        ("receipt_id".to_string(), ctx.receipt_id.clone()),
        ("actor_name".to_string(), ctx.actor_name.clone()),
        ("actor_email".to_string(), ctx.actor_email.clone()),
        ("timestamp".to_string(), ctx.timestamp.clone()),
        ("verified".to_string(), ctx.verified.clone()),
        ("risk_level".to_string(), ctx.risk_level.clone()),
        ("files_changed".to_string(), ctx.files_changed.clone()),
        ("workspace_root".to_string(), ctx.workspace_root.clone()),
        ("hook_name".to_string(), ctx.hook_name.clone()),
        ("hook_phase".to_string(), ctx.hook_phase.clone()),
    ])
}

fn hook_env(ctx: &HookContext) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert("DRAFT_HOOK_NAME".to_string(), ctx.hook_name.clone());
    env.insert("DRAFT_HOOK_PHASE".to_string(), ctx.hook_phase.clone());
    env.insert(
        "DRAFT_WORKSPACE_ROOT".to_string(),
        ctx.workspace_root.clone(),
    );
    env.insert("DRAFT_RECEIPT_ID".to_string(), ctx.receipt_id.clone());
    env.insert(
        "DRAFT_CHANGE_PACK_ID".to_string(),
        ctx.changepack_id.clone(),
    );
    env.insert("DRAFT_ACTOR_NAME".to_string(), ctx.actor_name.clone());
    env.insert("DRAFT_ACTOR_EMAIL".to_string(), ctx.actor_email.clone());
    for (k, v) in &ctx.vars {
        env.insert(format!("DRAFT_VAR_{}", k.to_ascii_uppercase()), v.clone());
    }
    env
}

pub fn parse_hook_vars(values: Vec<String>) -> DraftResult<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for item in values {
        if item.starts_with('-') {
            return Err(DraftError::invalid_config(
                "normal Draft flags are not allowed after --var",
            ));
        }
        let (key, value) = item
            .split_once('=')
            .ok_or_else(|| DraftError::invalid_config("--var entries must be key=value"))?;
        if !valid_var_name(key) {
            return Err(DraftError::invalid_config(format!(
                "invalid hook variable name '{key}'"
            )));
        }
        if builtin_placeholder_names().contains(key) {
            return Err(DraftError::invalid_config(format!(
                "hook variable '{key}' overrides a built-in placeholder"
            )));
        }
        out.insert(key.to_string(), value.to_string());
    }
    Ok(out)
}

fn valid_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn builtin_placeholder_names() -> BTreeSet<&'static str> {
    BTreeSet::from([
        "message",
        "title",
        "description",
        "task_id",
        "run_id",
        "changepack_id",
        "receipt_id",
        "actor_name",
        "actor_email",
        "timestamp",
        "verified",
        "risk_level",
        "files_changed",
        "workspace_root",
        "hook_name",
        "hook_phase",
    ])
}

fn interpolate_lenient(template: &str, values: &BTreeMap<String, String>) -> String {
    let mut out = template.to_string();
    for (k, v) in values {
        out = out.replace(&format!("{{{{{k}}}}}"), v);
    }
    out
}

fn interpolate_strict(
    template: &str,
    values: &BTreeMap<String, String>,
) -> Result<String, HookFailure> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or_else(|| HookFailure {
            message: "unclosed hook placeholder".to_string(),
        })?;
        let name = &after[..end];
        let value = values.get(name).ok_or_else(|| HookFailure {
            message: format!("missing hook placeholder '{{{{{name}}}}}'"),
        })?;
        out.push_str(value);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn shell(command: &str, cwd: &Path) -> std::io::Result<std::process::Output> {
    shell_with_env(command, cwd, &BTreeMap::new())
}

fn shell_with_env(
    command: &str,
    cwd: &Path,
    env: &BTreeMap<String, String>,
) -> std::io::Result<std::process::Output> {
    if cfg!(windows) {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", command])
            .current_dir(cwd)
            .envs(env)
            .output()
    } else {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", command])
            .current_dir(cwd)
            .envs(env)
            .output()
    }
}

fn default_shell() -> String {
    if cfg!(windows) {
        "cmd.exe /C".to_string()
    } else {
        "sh -c".to_string()
    }
}

fn command_hash(shell: &str, cwd: &Path, command: &str, rendered: &str) -> String {
    sha256_hex(format!("{shell}\n{}\n{command}\n{rendered}", cwd.display()).as_bytes())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))
}

fn hash_json<T: Serialize>(value: &T) -> DraftResult<String> {
    let value = serde_json::to_value(value).map_err(json_err)?;
    Ok(sha256_hex(canonical_json(&value).as_bytes()))
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(v) => v.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string()),
        Value::Array(items) => {
            let inner = items
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",");
            format!("[{inner}]")
        }
        Value::Object(map) => {
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let inner = entries
                .into_iter()
                .map(|(k, v)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_else(|_| "\"\"".to_string()),
                        canonical_json(v)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{inner}}}")
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

fn json_err(e: serde_json::Error) -> DraftError {
    DraftError::storage(format!("JSON error: {e}"))
}

impl From<serde_json::Error> for DraftError {
    fn from(e: serde_json::Error) -> Self {
        json_err(e)
    }
}
