//! User-scoped project registry used by global draftd and Doctor.

use crate::common::{now, Timestamp};
use crate::error::{DraftError, DraftResult};
use crate::fsutil::{ensure_dir, write_atomic};
use crate::home::GlobalHome;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRegistryEntry {
    pub schema_version: String,
    pub project_id: String,
    pub name: String,
    pub repository_path: String,
    pub storage_path: String,
    pub created_at: Timestamp,
    pub last_seen_at: Timestamp,
    pub last_stable_head: Option<String>,
    pub draft_version: String,
    pub health: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryIssue {
    pub project_id: String,
    pub kind: String,
    pub path: String,
    pub fixable: bool,
}

pub struct ProjectRegistry {
    path: PathBuf,
}
impl ProjectRegistry {
    pub fn global() -> DraftResult<Self> {
        let home = GlobalHome::locate()?;
        Ok(Self {
            path: home.root().join("registry/projects.jsonl"),
        })
    }
    pub fn list(&self) -> DraftResult<Vec<ProjectRegistryEntry>> {
        if !self.path.exists() {
            return Ok(vec![]);
        }
        let content = fs::read_to_string(&self.path)?;
        let mut out: Vec<ProjectRegistryEntry> = Vec::new();
        for (i, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            out.push(serde_json::from_str(line).map_err(|e| {
                DraftError::storage(format!("registry line {} is invalid: {e}", i + 1))
            })?);
        }
        out.sort_by(|a, b| a.project_id.cmp(&b.project_id));
        Ok(out)
    }
    pub fn upsert(
        &self,
        project_id: &str,
        root: &Path,
        stable: Option<String>,
    ) -> DraftResult<ProjectRegistryEntry> {
        let canonical = root
            .canonicalize()
            .map_err(|e| DraftError::storage(format!("cannot register {}: {e}", root.display())))?;
        let mut entries = self.list()?;
        let at = now();
        let existing = entries.iter().find(|e| e.project_id == project_id).cloned();
        let entry = ProjectRegistryEntry {
            schema_version: crate::DRAFT_SCHEMA_VERSION.into(),
            project_id: project_id.into(),
            name: canonical
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("project")
                .into(),
            repository_path: canonical.to_string_lossy().into(),
            storage_path: canonical.join(".draft").to_string_lossy().into(),
            created_at: existing.as_ref().map(|e| e.created_at).unwrap_or(at),
            last_seen_at: at,
            last_stable_head: stable,
            draft_version: crate::DRAFT_VERSION.into(),
            health: "healthy".into(),
        };
        entries
            .retain(|e| e.project_id != project_id && e.repository_path != entry.repository_path);
        entries.push(entry.clone());
        self.write(&entries)?;
        Ok(entry)
    }
    pub fn remove(&self, project_id: &str) -> DraftResult<bool> {
        let mut entries = self.list()?;
        let before = entries.len();
        entries.retain(|e| e.project_id != project_id);
        self.write(&entries)?;
        Ok(entries.len() != before)
    }
    pub fn inspect(&self) -> DraftResult<Vec<RegistryIssue>> {
        let entries = self.list()?;
        let mut issues = Vec::new();
        let mut paths = BTreeMap::<String, String>::new();
        for e in entries {
            let root = Path::new(&e.repository_path);
            if !root.exists() {
                issues.push(RegistryIssue {
                    project_id: e.project_id.clone(),
                    kind: "path_missing".into(),
                    path: e.repository_path.clone(),
                    fixable: true,
                });
            } else if !Path::new(&e.storage_path).is_dir() {
                issues.push(RegistryIssue {
                    project_id: e.project_id.clone(),
                    kind: "draft_missing".into(),
                    path: e.storage_path.clone(),
                    fixable: true,
                });
            }
            if let Some(other) = paths.insert(e.repository_path.clone(), e.project_id.clone()) {
                issues.push(RegistryIssue {
                    project_id: e.project_id,
                    kind: format!("duplicate_of:{other}"),
                    path: e.repository_path,
                    fixable: true,
                });
            }
        }
        Ok(issues)
    }
    pub fn fix_stale(&self) -> DraftResult<Vec<RegistryIssue>> {
        let issues = self.inspect()?;
        let stale: std::collections::BTreeSet<_> = issues
            .iter()
            .filter(|i| i.fixable)
            .map(|i| i.project_id.clone())
            .collect();
        let mut entries = self.list()?;
        entries.retain(|e| !stale.contains(&e.project_id));
        self.write(&entries)?;
        Ok(issues)
    }
    fn write(&self, entries: &[ProjectRegistryEntry]) -> DraftResult<()> {
        if let Some(parent) = self.path.parent() {
            ensure_dir(parent)?;
        }
        let mut sorted = entries.to_vec();
        sorted.sort_by(|a, b| a.project_id.cmp(&b.project_id));
        let mut bytes = Vec::new();
        for e in sorted {
            bytes.extend_from_slice(
                serde_json::to_string(&e)
                    .map_err(|x| DraftError::storage(x.to_string()))?
                    .as_bytes(),
            );
            bytes.push(b'\n');
        }
        write_atomic(&self.path, &bytes)
    }
}
