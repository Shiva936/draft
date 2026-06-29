//! Local identity resolution and persistence.
//!
//! Resolution order (FR-ID-002): workspace `.draft/identity.json` overrides the
//! user-global `~/.config/draft/identity.toml`; if neither exists, a best-effort
//! actor is derived from the environment, falling back to `Unknown` (never
//! crashes — FR-ID-001).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::common::ActorId;
use crate::error::DraftResult;
use crate::fsutil::{read_json, read_toml, write_json};

use super::actor::{ActorKind, ActorRef};

/// On-disk identity record (`.draft/identity.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityRecord {
    pub id: String,
    pub kind: ActorKindRepr,
    pub display_name: String,
    #[serde(default)]
    pub email: Option<String>,
}

/// Serialized representation of [`ActorKind`] for config files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActorKindRepr {
    Human,
    Agent,
    Service,
    Unknown,
}

impl From<ActorKindRepr> for ActorKind {
    fn from(r: ActorKindRepr) -> Self {
        match r {
            ActorKindRepr::Human => ActorKind::Human,
            ActorKindRepr::Agent => ActorKind::Agent,
            ActorKindRepr::Service => ActorKind::Service,
            ActorKindRepr::Unknown => ActorKind::Unknown,
        }
    }
}

impl From<&IdentityRecord> for ActorRef {
    fn from(r: &IdentityRecord) -> Self {
        ActorRef {
            id: ActorId::new(r.id.clone()),
            kind: r.kind.into(),
            display_name: r.display_name.clone(),
        }
    }
}

/// User-global identity file shape (`~/.config/draft/identity.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserIdentityFile {
    user: UserSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserSection {
    name: String,
    #[serde(default)]
    email: Option<String>,
}

/// Resolve the current actor for a workspace whose `.draft/` is at `draft_dir`.
pub fn resolve_actor(draft_dir: &Path) -> ActorRef {
    // 1. Workspace identity.
    let ws_path = draft_dir.join("identity.json");
    if ws_path.exists() {
        if let Ok(rec) = read_json::<IdentityRecord>(&ws_path) {
            return (&rec).into();
        }
    }
    // 2. User-global identity.
    if let Some(cfg) = user_config_dir() {
        let p = cfg.join("draft").join("identity.toml");
        if p.exists() {
            if let Ok(file) = read_toml::<UserIdentityFile>(&p) {
                return ActorRef {
                    id: ActorId::new(format!("act_{}", slug(&file.user.name))),
                    kind: ActorKind::Human,
                    display_name: file.user.name,
                };
            }
        }
    }
    // 3. Environment fallback.
    if let Ok(name) = std::env::var("USER").or_else(|_| std::env::var("USERNAME")) {
        if !name.is_empty() {
            return ActorRef {
                id: ActorId::new(format!("act_{}", slug(&name))),
                kind: ActorKind::Human,
                display_name: name,
            };
        }
    }
    // 4. Never crash.
    ActorRef::unknown()
}

/// Persist a workspace identity record.
pub fn save_workspace_identity(draft_dir: &Path, rec: &IdentityRecord) -> DraftResult<()> {
    write_json(&draft_dir.join("identity.json"), rec)
}

fn user_config_dir() -> Option<std::path::PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(std::path::PathBuf::from(xdg));
        }
    }
    std::env::var("HOME")
        .ok()
        .map(|h| std::path::PathBuf::from(h).join(".config"))
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}
