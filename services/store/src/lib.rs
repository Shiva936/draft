//! User-local service store (`~/.local/state/draft/`).
//!
//! Holds runtime caches and the workspace registry. This is **not** the source
//! of portable project truth — that lives in each workspace's `.draft/`
//! (FR-SVC-007). Stored as JSON with atomic write-then-rename.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub fn state_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("draft");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".local/state/draft")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRecord {
    pub id: String,
    pub path: String,
    pub provider_id: String,
    pub registered_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceIndex {
    pub workspaces: Vec<WorkspaceRecord>,
}

/// The service store rooted at `~/.local/state/draft/` (or a custom root for
/// tests).
pub struct ServiceStore {
    root: PathBuf,
}

impl ServiceStore {
    pub fn open_default() -> Self {
        ServiceStore::open(state_dir())
    }

    pub fn open(root: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(root.join("logs"));
        ServiceStore { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("workspace-index.json")
    }

    pub fn load_index(&self) -> WorkspaceIndex {
        std::fs::read_to_string(self.index_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save_index(&self, index: &WorkspaceIndex) -> std::io::Result<()> {
        write_atomic(
            &self.index_path(),
            serde_json::to_vec_pretty(index)?.as_slice(),
        )
    }

    /// Register (or update) a workspace by path.
    pub fn register(&self, record: WorkspaceRecord) -> std::io::Result<()> {
        let mut index = self.load_index();
        index.workspaces.retain(|w| w.path != record.path);
        index.workspaces.push(record);
        self.save_index(&index)
    }

    pub fn list(&self) -> Vec<WorkspaceRecord> {
        self.load_index().workspaces
    }

    pub fn remove(&self, path: &str) -> std::io::Result<()> {
        let mut index = self.load_index();
        index.workspaces.retain(|w| w.path != path);
        self.save_index(&index)
    }

    /// Append a line to the service log.
    pub fn log(&self, line: &str) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("logs").join("draftd.log"))
        {
            let _ = writeln!(f, "[{}] {}", chrono::Utc::now().to_rfc3339(), line);
        }
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}
