//! Real Draft protocol adapters (PRD §9.20, TDD §41–44).
//!
//! Three concrete adapters expose Draft over external protocols, each with a
//! real command path, real config under `~/.draft/adapters/<name>/`, a security
//! boundary, and event/receipt emission through `draft-core` (never a bypass):
//!
//! - [`mcp`]  — Model Context Protocol server (JSON-RPC over stdio) exposing
//!   *safe* operations to AI tools; dangerous operations (save/rollback/approve)
//!   are refused and must go through explicit human approval.
//! - [`acp`]  — approval-workflow operations (request/approve/reject/list).
//! - [`a2a`]  — candidate/actor coordination (register/link/history).
//!
//! The AG-UI adapter is the `draft-agui` cockpit crate.

pub mod a2a;
pub mod acp;
pub mod mcp;

use draft_core::home::GlobalHome;
use std::path::PathBuf;

/// Ensure an adapter's config directory exists under the global store and return
/// it. Adapters read/write only here for configuration.
pub fn ensure_adapter_config(name: &str) -> Result<PathBuf, String> {
    let home = GlobalHome::locate().map_err(|e| e.to_string())?;
    let dir = home.adapter_dir(name);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create adapter config {name}: {e}"))?;
    Ok(dir)
}
