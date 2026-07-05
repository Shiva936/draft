//! A2A adapter — candidate/actor coordination (TDD §43).
//!
//! Registers change-producing candidates in the global registry and links them
//! to packs. It records provenance only; it never bypasses local verification.

use draft_core::home::GlobalHome;
use draft_core::identity::global::{self, CandidateKind};
use serde_json::{json, Value};
use std::path::Path;

/// An A2A operation.
pub enum A2aOp<'a> {
    /// Register (or return existing) a candidate in the global registry.
    RegisterCandidate {
        name: &'a str,
        kind: &'a str,
        provider: &'a str,
    },
    /// List all registered candidates.
    ListCandidates,
    /// Record that a candidate produced a pack (provenance link).
    Link {
        candidate: &'a str,
        pack_id: &'a str,
    },
}

fn parse_kind(s: &str) -> CandidateKind {
    match s {
        "human" => CandidateKind::Human,
        "ai" => CandidateKind::Ai,
        "tool" => CandidateKind::Tool,
        "service" => CandidateKind::Service,
        _ => CandidateKind::Unknown,
    }
}

/// Run an A2A operation. `root` is the workspace (used for link provenance).
pub fn run(root: &Path, op: A2aOp<'_>) -> Result<Value, String> {
    let dir = crate::ensure_adapter_config("a2a")?;
    let home = GlobalHome::locate().map_err(|e| e.to_string())?;
    match op {
        A2aOp::RegisterCandidate {
            name,
            kind,
            provider,
        } => {
            let rec = global::register_candidate(&home, name, parse_kind(kind), provider)
                .map_err(|e| e.message)?;
            serde_json::to_value(rec).map_err(|e| e.to_string())
        }
        A2aOp::ListCandidates => {
            let list = global::list_candidates(&home).map_err(|e| e.message)?;
            serde_json::to_value(list).map_err(|e| e.to_string())
        }
        A2aOp::Link { candidate, pack_id } => {
            // Provenance links are appended to the global a2a adapter config
            // (`~/.draft/adapters/a2a/links.json`), tagged with the workspace
            // they came from; they never grant trust by themselves.
            let links_path = dir.join("links.json");
            let mut links: Vec<Value> = std::fs::read(&links_path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            links.push(json!({
                "candidate": candidate,
                "pack_id": pack_id,
                "workspace": root.display().to_string(),
                "at": draft_core::common::now().to_rfc3339(),
            }));
            std::fs::write(
                &links_path,
                serde_json::to_vec_pretty(&links).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            Ok(json!({"linked": {"candidate": candidate, "pack_id": pack_id}}))
        }
    }
}
