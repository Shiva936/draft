//! Diagnosis: what is wrong, and what would resolve it.
//!
//! Split out of `app/mod.rs`; these are `App` methods and behave
//! identically to when they lived there.

use super::*;

impl App {
    /// `draft doctor`: validate the global store and (if present) the project
    /// store for the current directory.
    pub fn doctor(&self, cwd: &Path) -> DraftResult<DoctorReport> {
        let global = self.doctor_global_scope()?;
        let project = match self.open(cwd) {
            Ok(ws) => Some(self.doctor_project_scope(&ws)?),
            Err(error) if error.kind == DraftErrorKind::WorkspaceNotFound => None,
            Err(error) => Some(DoctorScope {
                label: "project".to_string(),
                root: cwd.display().to_string(),
                exists: cwd.join(DRAFT_DIR).exists(),
                hidden: crate::support::hidden::is_hidden(&cwd.join(DRAFT_DIR)),
                checks: vec![DoctorCheck::fail_error("contract-open", error)],
            }),
        };
        Ok(DoctorReport { global, project })
    }

    /// `draft doctor --global`: validate only the global store.
    pub fn doctor_global(&self) -> DraftResult<DoctorReport> {
        Ok(DoctorReport {
            global: self.doctor_global_scope()?,
            project: None,
        })
    }

    fn doctor_global_scope(&self) -> DraftResult<DoctorScope> {
        let home = crate::project::home::DraftGlobalStore::locate()?;
        let exists = home.exists();
        let mut checks = Vec::new();
        if exists {
            for (name, check) in [
                (
                    "retired-profile-location",
                    crate::trust::identity::reject_retired_profile_state(None),
                ),
                (
                    "retired-actor-profile",
                    crate::trust::identity::global::reject_retired_actor_profile(&home),
                ),
                (
                    "retired-config-namespace",
                    crate::project::config::reject_retired_profile_config(&home.config_toml()),
                ),
            ] {
                match check {
                    Ok(()) => {
                        checks.push(DoctorCheck::ok(name, "unsupported profile state absent"))
                    }
                    Err(error) => checks.push(DoctorCheck::fail_error(name, error)),
                }
            }
            checks.push(bool_check(
                "identity",
                home.actor_json().exists(),
                "actor.json present",
                "actor.json missing — run `draft init --global`",
            ));
            checks.push(bool_check(
                "signing-key",
                home.signing_key().exists(),
                "signing key present",
                "signing key missing — run `draft init --global`",
            ));
            checks.push(bool_check(
                "keys-dir",
                home.keys_dir().is_dir(),
                "keys/ present",
                "keys/ missing",
            ));
            checks.push(bool_check(
                "default-policy",
                home.default_policy_toml().exists(),
                "default policy present",
                "default policy missing",
            ));
            match crate::activity::GlobalAuditLog::global().and_then(|audit| audit.verify()) {
                Ok(count) => checks.push(DoctorCheck::ok(
                    "global-audit",
                    format!("{count} hash-chained audit records verified"),
                )),
                Err(error) => checks.push(DoctorCheck::fail_error("global-audit", error)),
            }
            #[cfg(unix)]
            checks.push(key_perms_check(&home.signing_key()));
        } else {
            checks.push(DoctorCheck::fail(
                "exists",
                "global store missing — run `draft init --global`",
            ));
        }
        Ok(DoctorScope {
            label: "global".to_string(),
            root: home.root().display().to_string(),
            exists,
            hidden: crate::support::hidden::is_hidden(home.root()),
            checks,
        })
    }

    pub(crate) fn doctor_project_scope(&self, ws: &Workspace) -> DraftResult<DoctorScope> {
        let paths = crate::project::layout::DraftLayout::for_root(&ws.root);
        let mut checks = Vec::new();
        checks.push(bool_check(
            "workspace-json",
            paths.project_json().exists(),
            "project.json present",
            "project.json missing",
        ));
        // Activity chain integrity (reuses the existing verified replay).
        match self.verify_events(&ws.root) {
            Ok(_) => checks.push(DoctorCheck::ok(
                "activity-chain",
                "Activity hash chain intact",
            )),
            Err(e) => checks.push(DoctorCheck::fail_error("activity-chain", e)),
        }
        // Receipts and the transparency chain they are entered in.
        match crate::read_model::integrity::verify_all(&ws.layout, &ws.workspace_id) {
            Ok(v) => {
                checks.push(bool_check(
                    "activity-log",
                    v.activity_chain_ok,
                    format!("{} Activity events verified", v.activity_count),
                    "the Activity log does not verify",
                ));
                checks.push(bool_check(
                    "transparency-chain",
                    v.transparency_ok,
                    format!("{} transparency entries verified", v.transparency_count),
                    "transparency chain broken",
                ));
                let bad = v.receipts.iter().filter(|r| !r.ok).count();
                checks.push(bool_check(
                    "receipts",
                    bad == 0,
                    format!("{} receipts verified", v.receipts.len()),
                    format!("{bad} receipt(s) did not verify"),
                ));
            }
            Err(e) => checks.push(DoctorCheck::fail_error("receipts", e)),
        }
        for (name, dir) in [
            ("events-dir", paths.events_dir()),
            ("receipts-dir", paths.receipts_dir()),
            ("transparency-dir", paths.transparency_dir()),
            ("change-packs-dir", paths.change_packs_content_dir()),
            ("recovery-dir", paths.recovery_dir()),
        ] {
            checks.push(bool_check(
                name,
                dir.is_dir(),
                format!("{} present", dir.display()),
                format!("{} missing", dir.display()),
            ));
        }
        match crate::execution::operation::RecoveryStore::for_root(&ws.root).recoverable() {
            Ok(entries) if entries.is_empty() => {
                checks.push(DoctorCheck::ok("recovery", "no interrupted operations"))
            }
            Ok(entries) => checks.push(DoctorCheck::fail(
                "recovery",
                format!(
                    "{} interrupted operation(s) need recovery: {}",
                    entries.len(),
                    entries
                        .iter()
                        .map(|entry| format!("{}:{}", entry.recovery_id, entry.operation))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )),
            Err(e) => checks.push(DoctorCheck::fail_error("recovery", e)),
        }
        Ok(DoctorScope {
            label: "project".to_string(),
            root: ws.root.display().to_string(),
            exists: true,
            hidden: crate::support::hidden::is_hidden(paths.draft_dir()),
            checks,
        })
    }

    pub fn doctor_index(&self, cwd: &Path, refresh: bool) -> DraftResult<Value> {
        let ws = self.open(cwd)?;
        if refresh || !ws.layout.index_file().exists() {
            rebuild_index(&ws)?;
            crate::task::TaskStore::for_root(&ws.root).rebuild_index()?;
        }
        let paths = crate::project::layout::DraftLayout::for_root(&ws.root);
        let indexes = vec![
            index_status("file", ws.layout.index_file(), &[ws.layout.snapshots_dir()])?,
            index_status(
                "element",
                paths.impact_index_db(),
                std::slice::from_ref(&ws.root),
            )?,
            index_status("task", paths.task_name_index(), &[paths.tasks_dir()])?,
            index_status(
                "change-pack",
                paths.change_pack_graph_index(),
                &[paths.change_packs_content_dir()],
            )?,
            index_status(
                "receipt",
                paths.receipts_dir().join("index.json"),
                &[paths.receipts_dir()],
            )?,
            index_status(
                "search",
                paths.indexes_dir().join("search.json"),
                std::slice::from_ref(&ws.root),
            )?,
            index_status("activity", paths.activity_index(), &[paths.activity_log()])?,
        ];
        let state = if indexes.iter().any(|i| i["state"] == "failed") {
            "failed"
        } else if indexes.iter().any(|i| i["state"] == "missing") {
            "missing"
        } else if indexes.iter().any(|i| i["state"] == "stale") {
            "stale"
        } else {
            "fresh"
        };
        Ok(serde_json::json!({
            "state": state,
            "scope": "project",
            "refreshed": refresh,
            "indexes": indexes,
        }))
    }

    pub fn doctor_index_global(&self, refresh: bool) -> DraftResult<Value> {
        let home = crate::project::home::DraftGlobalStore::locate()?;
        if refresh {
            home.create_all()?;
        }
        let indexes = vec![
            index_status(
                "registry",
                home.registry_dir().join("projects.index"),
                &[home.registry_dir().join("projects.jsonl")],
            )?,
            index_status(
                "receipt",
                home.global_receipt_index(),
                &[home.receipts_dir()],
            )?,
            index_status(
                "candidate",
                home.indexes_dir().join("candidates.json"),
                &[home.candidates_json()],
            )?,
            index_status(
                "activity",
                home.indexes_dir().join("activity.json"),
                &[home.logs_dir()],
            )?,
        ];
        let state = if indexes.iter().any(|i| i["state"] == "failed") {
            "failed"
        } else if indexes.iter().any(|i| i["state"] == "missing") {
            "missing"
        } else if indexes.iter().any(|i| i["state"] == "stale") {
            "stale"
        } else {
            "fresh"
        };
        Ok(serde_json::json!({
            "state": state,
            "scope": "global",
            "root": home.root(),
            "refreshed": refresh,
            "indexes": indexes,
        }))
    }
}
