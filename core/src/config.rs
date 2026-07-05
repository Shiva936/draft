//! Config resolution with strict precedence (PRD §9.4, TDD §10.1).
//!
//! Precedence, highest first:
//!   1. CLI flags (`--set key=value` style overrides)
//!   2. project `<root>/.draft/config.toml`
//!   3. global `~/.draft/config.toml`
//!   4. built-in safe defaults
//!
//! Values are addressed by dotted keys (e.g. `risk.block_on_critical`). A read
//! walks the layers top-down and returns the first hit; a write targets exactly
//! one layer (project by default, global with `--global`).

use crate::error::DraftResult;
use crate::fsutil;
use std::collections::BTreeMap;
use std::path::Path;
use toml::Value;

/// Which layer a config write targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigScope {
    Project,
    Global,
}

/// A fully layered, resolved configuration.
pub struct ConfigResolver {
    cli: BTreeMap<String, String>,
    project: Value,
    global: Value,
    defaults: Value,
}

impl ConfigResolver {
    /// Load project and global config files (missing files → empty tables) and
    /// layer them over the built-in defaults.
    pub fn load(project_config: Option<&Path>, global_config: Option<&Path>) -> Self {
        ConfigResolver {
            cli: BTreeMap::new(),
            project: load_table(project_config),
            global: load_table(global_config),
            defaults: builtin_defaults(),
        }
    }

    /// Add a CLI override (highest precedence). `spec` is `key=value`.
    pub fn with_cli_override(mut self, key: &str, value: &str) -> Self {
        self.cli.insert(key.to_string(), value.to_string());
        self
    }

    /// Resolve a dotted key across all layers, returning its string form.
    pub fn get(&self, key: &str) -> Option<String> {
        if let Some(v) = self.cli.get(key) {
            return Some(v.clone());
        }
        for layer in [&self.project, &self.global, &self.defaults] {
            if let Some(v) = get_dotted(layer, key) {
                return Some(scalar_to_string(v));
            }
        }
        None
    }

    /// Report which layer a key resolved from (for `doctor`/diagnostics).
    pub fn source_of(&self, key: &str) -> Option<&'static str> {
        if self.cli.contains_key(key) {
            return Some("cli");
        }
        if get_dotted(&self.project, key).is_some() {
            return Some("project");
        }
        if get_dotted(&self.global, key).is_some() {
            return Some("global");
        }
        if get_dotted(&self.defaults, key).is_some() {
            return Some("default");
        }
        None
    }
}

/// Write `key = value` into the config file for `scope`, creating it if needed.
pub fn set_value(path: &Path, key: &str, value: &str) -> DraftResult<()> {
    let mut table = load_table(Some(path));
    set_dotted(&mut table, key, parse_scalar(value));
    fsutil::write_toml(path, &table)
}

/// Read `key` from a single config file (used by `config get` when a scope is
/// pinned). Returns `None` if absent.
pub fn get_value(path: &Path, key: &str) -> Option<String> {
    get_dotted(&load_table(Some(path)), key).map(scalar_to_string)
}

// ---- Built-in defaults ---------------------------------------------------

fn builtin_defaults() -> Value {
    // Kept intentionally small; each key has a safe, offline value.
    let toml = r#"
[core]
schema_version = "0.3.2"

[risk]
block_on_critical = true

[import]
require_local_verify = true
max_artifact_bytes = 104857600

[agui]
bind = "127.0.0.1"
port = 4317
"#;
    toml::from_str(toml).expect("built-in defaults must parse")
}

// ---- Dotted-key helpers over toml::Value ---------------------------------

fn load_table(path: Option<&Path>) -> Value {
    match path {
        Some(p) if p.exists() => match std::fs::read_to_string(p) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|_| empty_table()),
            Err(_) => empty_table(),
        },
        _ => empty_table(),
    }
}

fn empty_table() -> Value {
    Value::Table(Default::default())
}

fn get_dotted<'a>(root: &'a Value, key: &str) -> Option<&'a Value> {
    let mut cur = root;
    for part in key.split('.') {
        cur = cur.as_table()?.get(part)?;
    }
    Some(cur)
}

fn set_dotted(root: &mut Value, key: &str, value: Value) {
    if !root.is_table() {
        *root = empty_table();
    }
    let parts: Vec<&str> = key.split('.').collect();
    let mut cur = root;
    for part in &parts[..parts.len() - 1] {
        let table = cur.as_table_mut().expect("ensured table");
        cur = table.entry(part.to_string()).or_insert_with(empty_table);
        if !cur.is_table() {
            *cur = empty_table();
        }
    }
    let last = parts[parts.len() - 1];
    cur.as_table_mut()
        .expect("ensured table")
        .insert(last.to_string(), value);
}

fn parse_scalar(s: &str) -> Value {
    if let Ok(b) = s.parse::<bool>() {
        return Value::Boolean(b);
    }
    if let Ok(i) = s.parse::<i64>() {
        return Value::Integer(i);
    }
    Value::String(s.to_string())
}

fn scalar_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Integer(i) => i.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Float(f) => f.to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_cli_over_project_over_global_over_default() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("project.toml");
        let glob = tmp.path().join("global.toml");
        set_value(&glob, "risk.block_on_critical", "false").unwrap();
        set_value(&proj, "risk.block_on_critical", "true").unwrap();

        let r = ConfigResolver::load(Some(&proj), Some(&glob));
        assert_eq!(r.get("risk.block_on_critical").as_deref(), Some("true"));
        assert_eq!(r.source_of("risk.block_on_critical"), Some("project"));

        let r = r.with_cli_override("risk.block_on_critical", "false");
        assert_eq!(r.get("risk.block_on_critical").as_deref(), Some("false"));
        assert_eq!(r.source_of("risk.block_on_critical"), Some("cli"));
    }

    #[test]
    fn falls_back_to_builtin_default() {
        let r = ConfigResolver::load(None, None);
        assert_eq!(r.get("core.schema_version").as_deref(), Some("0.3.2"));
        assert_eq!(r.source_of("core.schema_version"), Some("default"));
        assert_eq!(r.get("agui.port").as_deref(), Some("4317"));
    }

    #[test]
    fn set_then_get_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("config.toml");
        set_value(&proj, "custom.key", "hello").unwrap();
        assert_eq!(get_value(&proj, "custom.key").as_deref(), Some("hello"));
    }
}
