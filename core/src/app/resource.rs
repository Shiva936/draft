//! Reading, writing and comparing the resources a project holds.
//!
//! Split out of `app/mod.rs`; these are `App` methods and behave
//! identically to when they lived there.

use super::*;

impl App {
    /// Every resource in the project, with whatever installed extensions can
    /// say about each.
    ///
    /// Classification is attached but never conflated with state: `classes` is
    /// derived interpretation, and the entry lists every resource whether or not
    /// anything recognizes it.
    pub fn resource_tree(&self, cwd: &Path) -> DraftResult<Vec<ResourceEntry>> {
        let ws = self.open(cwd)?;
        let snapshot = self.observe(&ws)?;
        let contributions = self.active_contributions();
        let classification =
            crate::evidence::classification::classify_snapshot(&snapshot, &contributions);
        let classes = classification.by_resource();
        let protections = crate::project::protected::rules_for_project(
            &ws.root,
            &contributed_protections(&contributions),
        )?;

        let mut entries = Vec::with_capacity(snapshot.resources.len());
        for state in &snapshot.resources {
            let collisions: Vec<String> = classification
                .collisions
                .iter()
                .filter(|collision| collision.resource_id == state.resource_id)
                .map(|collision| collision.class_id.qualified())
                .collect();
            entries.push(ResourceEntry {
                // The resource is fully observed here, so a protection
                // predicating on media type, form or size applies too — not
                // only the ones that name a path.
                protected: crate::project::protected::matches_resource(
                    &protections,
                    &crate::extension::ResourceView {
                        locator_scheme: state.locator.scheme.as_str(),
                        locator_body: state.locator.body.as_str(),
                        media_type: state.media_type.as_deref(),
                        form: state.form,
                        attributes: &state.attributes,
                        content_size: state.content_size,
                    },
                )
                .is_some(),
                classes: classes
                    .get(&state.resource_id)
                    .map(|set| set.iter().map(|class| class.qualified()).collect())
                    .unwrap_or_default(),
                class_collisions: collisions,
                locator: state.locator.clone(),
                resource_id: state.resource_id.clone(),
                form: state.form,
                content_size: state.content_size,
            });
        }
        entries.sort_by(|left, right| left.locator.cmp(&right.locator));
        Ok(entries)
    }

    pub fn resource_workspace(&self, cwd: &Path) -> DraftResult<ResourceWorkspaceReport> {
        let ws = self.open(cwd)?;
        let resources = self.resource_tree(cwd)?.len();
        let pending_dir = crate::project::layout::DraftLayout::for_root(&ws.root)
            .workspaces_dir()
            .join("pending");
        let pending_edits = if pending_dir.exists() {
            list_with_extension(&pending_dir, "json")?.len()
        } else {
            0
        };
        Ok(ResourceWorkspaceReport {
            mode: if pending_edits > 0 {
                "task_edit".to_string()
            } else {
                "browse".to_string()
            },
            workspace_hash: self.workspace_hash(&ws.root)?,
            pending_edits,
            resources,
            status: if pending_edits > 0 {
                "needs_review"
            } else {
                "ready"
            }
            .to_string(),
        })
    }

    /// Read one resource's content as text.
    ///
    /// Bounded and scheme-checked: only the built-in filesystem adapter's
    /// locators are readable this way, and only as UTF-8. Another scheme's
    /// content goes through its adapter, never through here.
    pub fn resource_read(
        &self,
        cwd: &Path,
        locator: &ResourceLocator,
    ) -> DraftResult<ResourceContentView> {
        let ws = self.open(cwd)?;
        let rel = filesystem_relative(locator)?;
        crate::project::protected::ensure_allowed(&self.protections(&ws.root)?, &rel)?;
        let fs_path = safe_workspace_dest(&ws.root, &rel)?;
        let bytes = std::fs::read(&fs_path)
            .map_err(|e| DraftError::not_found(format!("cannot read {}: {e}", rel.as_str())))?;
        let content = String::from_utf8(bytes).map_err(|_| {
            DraftError::new(
                DraftErrorKind::ProtectedResourceAccess,
                format!("resource {} is not UTF-8 text", rel.as_str()),
            )
        })?;
        Ok(ResourceContentView {
            locator: locator.clone(),
            content,
            protected: false,
            workspace_hash: self.workspace_hash(&ws.root)?,
        })
    }

    pub fn resource_create(
        &self,
        cwd: &Path,
        locator: &ResourceLocator,
        content: &str,
    ) -> DraftResult<ResourceMutationReport> {
        let ws = self.open(cwd)?;
        let rel = checked_resource_path(&self.protections(&ws.root)?, locator)?;
        let dest = safe_workspace_dest(&ws.root, &rel)?;
        if dest.exists() {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!("resource already exists: {}", rel.as_str()),
            ));
        }
        if let Some(parent) = dest.parent() {
            ensure_dir(parent)?;
        }
        write_atomic(&dest, content.as_bytes())?;
        ws.events()?.append(
            crate::activity::EventKind::OperationExecuted,
            Some(rel.to_string()),
            serde_json::json!({ "locator": locator }),
        )?;
        Ok(ResourceMutationReport {
            locator: locator.clone(),
            previous_locator: None,
            backup_path: None,
            workspace_hash: self.workspace_hash(&ws.root)?,
            action: "created".to_string(),
        })
    }

    pub fn resource_relocate(
        &self,
        cwd: &Path,
        from: &ResourceLocator,
        to: &ResourceLocator,
    ) -> DraftResult<ResourceMutationReport> {
        let ws = self.open(cwd)?;
        let from_rel = checked_resource_path(&self.protections(&ws.root)?, from)?;
        let to_rel = checked_resource_path(&self.protections(&ws.root)?, to)?;
        let from_path = safe_workspace_dest(&ws.root, &from_rel)?;
        let to_path = safe_workspace_dest(&ws.root, &to_rel)?;
        if !from_path.is_file() {
            return Err(DraftError::not_found(format!(
                "resource does not exist: {}",
                from_rel.as_str()
            )));
        }
        if to_path.exists() {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!("destination already exists: {}", to_rel.as_str()),
            ));
        }
        if let Some(parent) = to_path.parent() {
            ensure_dir(parent)?;
        }
        fs::rename(&from_path, &to_path).map_err(|e| {
            DraftError::storage(format!(
                "failed to relocate {} to {}: {e}",
                from_rel.as_str(),
                to_rel.as_str()
            ))
        })?;
        ws.events()?.append(
            crate::activity::EventKind::OperationExecuted,
            Some(to_rel.to_string()),
            serde_json::json!({ "from": from, "to": to }),
        )?;
        Ok(ResourceMutationReport {
            locator: to.clone(),
            previous_locator: Some(from.clone()),
            backup_path: None,
            workspace_hash: self.workspace_hash(&ws.root)?,
            action: "relocated".to_string(),
        })
    }

    pub fn resource_delete(
        &self,
        cwd: &Path,
        locator: &ResourceLocator,
    ) -> DraftResult<ResourceMutationReport> {
        let ws = self.open(cwd)?;
        let rel = checked_resource_path(&self.protections(&ws.root)?, locator)?;
        let dest = safe_workspace_dest(&ws.root, &rel)?;
        if !dest.is_file() {
            return Err(DraftError::not_found(format!(
                "resource does not exist: {}",
                rel.as_str()
            )));
        }
        let backup = editor_backup_path(&ws.root, &rel)?;
        if let Some(parent) = backup.parent() {
            ensure_dir(parent)?;
        }
        fs::copy(&dest, &backup)
            .map_err(|e| DraftError::storage(format!("failed to back up {}: {e}", rel.as_str())))?;
        fs::remove_file(&dest)
            .map_err(|e| DraftError::storage(format!("failed to delete {}: {e}", rel.as_str())))?;
        ws.events()?.append(
            crate::activity::EventKind::OperationExecuted,
            Some(rel.to_string()),
            serde_json::json!({ "locator": locator, "backup": backup.display().to_string() }),
        )?;
        Ok(ResourceMutationReport {
            locator: locator.clone(),
            previous_locator: None,
            backup_path: Some(backup.display().to_string()),
            workspace_hash: self.workspace_hash(&ws.root)?,
            action: "deleted".to_string(),
        })
    }

    pub fn resource_search(
        &self,
        cwd: &Path,
        query: &str,
        limit: usize,
    ) -> DraftResult<Vec<ResourceSearchHit>> {
        let ws = self.open(cwd)?;
        let query = query.trim();
        if query.is_empty() {
            return Ok(vec![]);
        }
        let mut hits = Vec::new();
        for entry in self.resource_tree(cwd)? {
            if entry.protected || entry.locator.scheme != crate::extension::FILE_SCHEME {
                continue;
            }
            let path = safe_workspace_dest(&ws.root, &WorkspacePath::new(&entry.locator.body))?;
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            let Ok(content) = String::from_utf8(bytes) else {
                continue;
            };
            for (index, line) in content.lines().enumerate() {
                if line.contains(query) {
                    hits.push(ResourceSearchHit {
                        locator: entry.locator.clone(),
                        line: (index + 1) as u32,
                        preview: crate::support::redaction::redact(line.trim()),
                    });
                    if hits.len() >= limit.max(1) {
                        return Ok(hits);
                    }
                }
            }
        }
        Ok(hits)
    }
}
