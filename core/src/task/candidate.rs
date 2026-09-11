//! Candidate profiles, capabilities, limits, and task presets.
//!
//! A candidate is any tool or person that can produce work for a task: an
//! agent command, a plain command, or a human editing through the editor.
//! Profiles are resolved with strict precedence:
//!
//!   1. project `.draft/config.toml` `[candidates.<name>]`
//!   2. global `~/.draft/config.toml` `[candidates.<name>]`
//!   3. built-in defaults (`manual`, `codex`, `claude`)
//!
//! Presets (`[tasks.presets.<name>]`) resolve the same way and fall back to
//! the built-in `fast`, `strict`, and `paranoid` presets.

use crate::support::common::{now, Timestamp};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use toml::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    Agent,
    Command,
    Manual,
    Human,
}

impl CandidateKind {
    pub fn parse(s: &str) -> DraftResult<Self> {
        match s {
            "agent" => Ok(CandidateKind::Agent),
            "command" => Ok(CandidateKind::Command),
            "manual" => Ok(CandidateKind::Manual),
            "human" => Ok(CandidateKind::Human),
            other => Err(DraftError::invalid_config(format!(
                "unknown candidate kind '{other}'"
            ))),
        }
    }

    /// Whether executions for this candidate run an external command.
    pub fn runs_command(&self) -> bool {
        matches!(self, CandidateKind::Agent | CandidateKind::Command)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CandidateCapabilities {
    pub can_plan: bool,
    /// Whether the candidate can change project state at all.
    pub can_edit: bool,
    /// Whether it can run the project's verification checks itself.
    ///
    /// Neutral: what a check *is* comes from the project's configuration or a
    /// contributed verification capability, not from this flag.
    pub can_verify: bool,
    pub can_read_index: bool,
    pub can_accept_task_contract: bool,
    pub supports_resume: bool,
    pub supports_streaming_logs: bool,
    /// Whether it returns proposed mutations rather than editing in place.
    ///
    /// A proposal is the safer shape — Draft authors the plan from it — but
    /// neither shape lets the candidate author authority metadata.
    pub proposes_mutations: bool,
    pub supports_change_output: bool,
    pub requires_shell: bool,
    pub requires_network: bool,
}

impl Default for CandidateCapabilities {
    fn default() -> Self {
        // Conservative defaults: a configured candidate can edit and accept a
        // task contract, everything else must be declared explicitly.
        CandidateCapabilities {
            can_plan: false,
            can_edit: true,
            can_verify: false,
            can_read_index: false,
            can_accept_task_contract: true,
            supports_resume: false,
            supports_streaming_logs: false,
            proposes_mutations: false,
            supports_change_output: false,
            requires_shell: false,
            requires_network: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicy {
    /// Network use is not declared; treated as denied for review purposes.
    #[default]
    Denied,
    /// Loopback-only network use is declared.
    Local,
    /// Full network use is declared.
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IsolationMode {
    /// Run in an isolated copy of the workspace under `.draft/runtime/`.
    #[default]
    Isolated,
    /// Run directly in the project working tree (manual/human flows).
    InPlace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct CandidateLimits {
    pub max_runtime_seconds: Option<u64>,
    pub max_files_changed: Option<u32>,
    /// A byte budget on what one candidate run may produce.
    ///
    /// Bytes rather than lines: Draft has no notion of a line, and a candidate
    /// may legitimately produce something that has none. An extension that cares
    /// about lines expresses that through a contributed reviewability metric.
    pub max_output_bytes_changed: Option<u64>,
    pub max_processes: Option<u32>,
    pub max_output_bytes: Option<u64>,
    pub network: NetworkPolicy,
    pub allowed_paths: Vec<String>,
    pub forbidden_paths: Vec<String>,
    pub env_allowlist: Vec<String>,
    pub command_allowlist: Vec<String>,
    pub isolation_mode: IsolationMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateProfile {
    pub schema_version: u32,
    pub name: String,
    pub kind: CandidateKind,
    /// Command template; `{{instruction}}` is replaced with the task goal or
    /// inline instruction. `None` for manual/human candidates.
    pub command: Option<String>,
    pub capabilities: CandidateCapabilities,
    pub limits: CandidateLimits,
    /// Where this profile was resolved from (`project-config`,
    /// `global-config`, `project-record`, `builtin`).
    pub source: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl crate::contracts::VersionedContract for CandidateProfile {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::CandidateProfile;
}

impl CandidateProfile {
    fn base(name: &str, kind: CandidateKind, command: Option<String>, source: &str) -> Self {
        let at = now();
        CandidateProfile {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::CandidateProfile,
            ),
            name: name.to_string(),
            kind,
            command,
            capabilities: CandidateCapabilities::default(),
            limits: CandidateLimits::default(),
            source: source.to_string(),
            created_at: at,
            updated_at: at,
        }
    }

    /// Validate that this candidate can take on the given execution shape.
    pub fn ensure_capability(&self, needed: &str) -> DraftResult<()> {
        let ok = match needed {
            "edit" => self.capabilities.can_edit,
            "plan" => self.capabilities.can_plan,
            "verify" => self.capabilities.can_verify,
            "resume" => self.capabilities.supports_resume,
            "task_contract" => self.capabilities.can_accept_task_contract,
            other => {
                return Err(DraftError::invalid_config(format!(
                    "unknown candidate capability '{other}'"
                )))
            }
        };
        if ok {
            Ok(())
        } else {
            Err(DraftError::new(
                DraftErrorKind::CandidateNotConfigured,
                format!(
                    "candidate '{}' does not declare the '{}' capability",
                    self.name, needed
                ),
            )
            .with_suggestion(format!(
                "set `{}` under [candidates.{}] in .draft/config.toml",
                capability_key(needed),
                self.name
            )))
        }
    }
}

fn capability_key(needed: &str) -> &'static str {
    match needed {
        "plan" => "can_plan",
        "verify" => "can_verify",
        "resume" => "supports_resume",
        "task_contract" => "can_accept_task_contract",
        _ => "can_edit",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidatePreset {
    pub schema_version: u32,
    pub name: String,
    pub candidates: Vec<String>,
    #[serde(default)]
    pub plan_first: bool,
    #[serde(default)]
    pub require_full_evidence: bool,
    #[serde(default)]
    pub prefer_smallest_valid_change: bool,
    pub source: String,
}

impl crate::contracts::VersionedContract for CandidatePreset {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::CandidatePreset;
}

fn builtin_presets() -> Vec<CandidatePreset> {
    let preset = |name: &str,
                  candidates: &[&str],
                  plan_first: bool,
                  require_full_evidence: bool,
                  prefer_smallest_valid_change: bool| CandidatePreset {
        schema_version: crate::contracts::current_version(
            crate::contracts::ContractId::CandidatePreset,
        ),
        name: name.into(),
        candidates: candidates.iter().map(|c| c.to_string()).collect(),
        plan_first,
        require_full_evidence,
        prefer_smallest_valid_change,
        source: "builtin".into(),
    };
    vec![
        preset("fast", &["codex"], false, false, false),
        preset("strict", &["codex", "claude"], true, true, false),
        preset("paranoid", &["codex", "claude", "cursor"], true, true, true),
    ]
}

fn builtin_profiles() -> Vec<CandidateProfile> {
    let mut manual = CandidateProfile::base("manual", CandidateKind::Manual, None, "builtin");
    manual.capabilities.can_plan = true;
    manual.capabilities.can_verify = true;
    manual.capabilities.can_accept_task_contract = false;
    manual.limits.isolation_mode = IsolationMode::InPlace;

    let mut human = CandidateProfile::base("human", CandidateKind::Human, None, "builtin");
    human.capabilities = manual.capabilities;
    human.limits.isolation_mode = IsolationMode::InPlace;

    let agent = |name: &str| {
        let mut p = CandidateProfile::base(
            name,
            CandidateKind::Agent,
            Some(format!("{name} {{{{instruction}}}}")),
            "builtin",
        );
        p.capabilities.can_plan = true;
        p.capabilities.requires_shell = true;
        p
    };
    vec![manual, human, agent("codex"), agent("claude")]
}

/// Resolves candidate profiles and presets across config layers.
pub struct CandidateRegistry {
    profiles: BTreeMap<String, CandidateProfile>,
    presets: BTreeMap<String, CandidatePreset>,
}

impl CandidateRegistry {
    pub fn load(project_config: Option<&Path>, global_config: Option<&Path>) -> DraftResult<Self> {
        let mut profiles: BTreeMap<String, CandidateProfile> = BTreeMap::new();
        let mut presets: BTreeMap<String, CandidatePreset> = BTreeMap::new();
        for p in builtin_profiles() {
            profiles.insert(p.name.clone(), p);
        }
        for p in builtin_presets() {
            presets.insert(p.name.clone(), p);
        }
        // Global first so project entries override.
        for (path, source) in [
            (global_config, "global-config"),
            (project_config, "project-config"),
        ] {
            let Some(path) = path else { continue };
            if !path.exists() {
                continue;
            }
            crate::project::config::read(path)?;
            let text = std::fs::read_to_string(path)?;
            let value = text.parse::<Value>().map_err(|error| {
                DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!(
                        "malformed candidate configuration {}: {error}",
                        path.display()
                    ),
                )
            })?;
            match value.get("schema_version") {
                Some(Value::Integer(version))
                    if u32::try_from(*version).is_ok_and(|version| {
                        crate::contracts::supports_version(
                            crate::contracts::ContractId::DraftConfig,
                            version,
                        )
                    }) => {}
                Some(Value::Integer(version)) => {
                    return Err(DraftError::new(
                        DraftErrorKind::UnsupportedSchema,
                        format!("unsupported candidate configuration schema {version}"),
                    ))
                }
                _ => {
                    return Err(DraftError::new(
                        DraftErrorKind::CorruptData,
                        format!(
                            "candidate configuration {} requires numeric schema_version = {}",
                            path.display(),
                            crate::contracts::current_version(
                                crate::contracts::ContractId::DraftConfig
                            )
                        ),
                    ))
                }
            }
            for (name, table) in config_tables(&value, &["candidates"]) {
                let existing = profiles.get(&name).cloned();
                profiles.insert(
                    name.clone(),
                    profile_from_table(&name, &table, source, existing)?,
                );
            }
            for (name, table) in config_tables(&value, &["tasks", "presets"]) {
                presets.insert(name.clone(), preset_from_table(&name, &table, source));
            }
        }
        Ok(CandidateRegistry { profiles, presets })
    }

    pub fn profiles(&self) -> Vec<CandidateProfile> {
        self.profiles.values().cloned().collect()
    }

    pub fn profile(&self, name: &str) -> DraftResult<CandidateProfile> {
        self.profiles.get(name).cloned().ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CandidateNotConfigured,
                format!("candidate '{name}' is not configured"),
            )
            .with_suggestion(format!(
                "add [candidates.{name}] to .draft/config.toml or run `draft change candidate add {name} -- <command>`"
            ))
        })
    }

    pub fn presets(&self) -> Vec<CandidatePreset> {
        self.presets.values().cloned().collect()
    }

    pub fn preset(&self, name: &str) -> DraftResult<CandidatePreset> {
        self.presets.get(name).cloned().ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CandidateNotConfigured,
                format!("task preset '{name}' is not configured"),
            )
            .with_suggestion(format!(
                "add [tasks.presets.{name}] to .draft/config.toml (builtins: fast, strict, paranoid)"
            ))
        })
    }
}

fn config_tables(root: &Value, path: &[&str]) -> Vec<(String, Value)> {
    let mut cur = root;
    for part in path {
        match cur.get(part) {
            Some(v) => cur = v,
            None => return Vec::new(),
        }
    }
    match cur.as_table() {
        Some(table) => table
            .iter()
            .filter(|(_, v)| v.is_table())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        None => Vec::new(),
    }
}

fn get_bool(table: &Value, key: &str, default: bool) -> bool {
    table.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn get_u64(table: &Value, key: &str) -> Option<u64> {
    table
        .get(key)
        .and_then(Value::as_integer)
        .and_then(|i| u64::try_from(i).ok())
}

fn get_u32(table: &Value, key: &str) -> Option<u32> {
    get_u64(table, key).and_then(|v| u32::try_from(v).ok())
}

fn get_str_list(table: &Value, key: &str) -> Vec<String> {
    table
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn profile_from_table(
    name: &str,
    table: &Value,
    source: &str,
    existing: Option<CandidateProfile>,
) -> DraftResult<CandidateProfile> {
    let base = existing
        .unwrap_or_else(|| CandidateProfile::base(name, CandidateKind::Command, None, source));
    let kind = table
        .get("kind")
        .and_then(Value::as_str)
        .map(CandidateKind::parse)
        .transpose()?
        .unwrap_or(base.kind);
    let command = table
        .get("command")
        .and_then(Value::as_str)
        .map(|c| {
            if c.contains("{{instruction}}") {
                c.to_string()
            } else {
                format!("{c} {{{{instruction}}}}")
            }
        })
        .or(base.command);
    let caps = CandidateCapabilities {
        can_plan: get_bool(table, "can_plan", base.capabilities.can_plan),
        can_edit: get_bool(table, "can_edit", base.capabilities.can_edit),
        can_verify: get_bool(table, "can_verify", base.capabilities.can_verify),
        can_read_index: get_bool(table, "can_read_index", base.capabilities.can_read_index),
        can_accept_task_contract: get_bool(
            table,
            "can_accept_task_contract",
            base.capabilities.can_accept_task_contract,
        ),
        supports_resume: get_bool(table, "supports_resume", base.capabilities.supports_resume),
        supports_streaming_logs: get_bool(
            table,
            "supports_streaming_logs",
            base.capabilities.supports_streaming_logs,
        ),
        proposes_mutations: get_bool(
            table,
            "proposes_mutations",
            base.capabilities.proposes_mutations,
        ),
        supports_change_output: get_bool(
            table,
            "supports_change_output",
            base.capabilities.supports_change_output,
        ),
        requires_shell: get_bool(table, "requires_shell", base.capabilities.requires_shell),
        requires_network: get_bool(
            table,
            "requires_network",
            base.capabilities.requires_network,
        ),
    };
    let network = match table.get("network").and_then(Value::as_str) {
        Some("local") => NetworkPolicy::Local,
        Some("full") => NetworkPolicy::Full,
        Some(_) => NetworkPolicy::Denied,
        None => base.limits.network,
    };
    let isolation_mode = match table.get("isolation").and_then(Value::as_str) {
        Some("in_place") | Some("in-place") => IsolationMode::InPlace,
        Some(_) => IsolationMode::Isolated,
        None => base.limits.isolation_mode,
    };
    let limits = CandidateLimits {
        max_runtime_seconds: get_u64(table, "max_runtime_seconds")
            .or(base.limits.max_runtime_seconds),
        max_files_changed: get_u32(table, "max_files_changed").or(base.limits.max_files_changed),
        max_output_bytes_changed: get_u64(table, "max_output_bytes_changed")
            .or(base.limits.max_output_bytes_changed),
        max_processes: get_u32(table, "max_processes").or(base.limits.max_processes),
        max_output_bytes: get_u64(table, "max_output_bytes").or(base.limits.max_output_bytes),
        network,
        allowed_paths: {
            let v = get_str_list(table, "allowed_paths");
            if v.is_empty() {
                base.limits.allowed_paths.clone()
            } else {
                v
            }
        },
        forbidden_paths: {
            let v = get_str_list(table, "forbidden_paths");
            if v.is_empty() {
                base.limits.forbidden_paths.clone()
            } else {
                v
            }
        },
        env_allowlist: {
            let v = get_str_list(table, "env_allowlist");
            if v.is_empty() {
                base.limits.env_allowlist.clone()
            } else {
                v
            }
        },
        command_allowlist: {
            let v = get_str_list(table, "command_allowlist");
            if v.is_empty() {
                base.limits.command_allowlist.clone()
            } else {
                v
            }
        },
        isolation_mode,
    };
    Ok(CandidateProfile {
        schema_version: crate::contracts::current_version(
            crate::contracts::ContractId::CandidateProfile,
        ),
        name: name.to_string(),
        kind,
        command,
        capabilities: caps,
        limits,
        source: source.to_string(),
        created_at: base.created_at,
        updated_at: now(),
    })
}

fn preset_from_table(name: &str, table: &Value, source: &str) -> CandidatePreset {
    CandidatePreset {
        schema_version: crate::contracts::current_version(
            crate::contracts::ContractId::CandidatePreset,
        ),
        name: name.to_string(),
        candidates: get_str_list(table, "candidates"),
        plan_first: get_bool(table, "plan_first", false),
        require_full_evidence: get_bool(table, "require_full_evidence", false),
        prefer_smallest_valid_change: get_bool(table, "prefer_smallest_valid_change", false),
        source: source.to_string(),
    }
}

/// Render a candidate command template into argv form.
pub fn render_command(template: &str, instruction: &str) -> Vec<String> {
    template
        .replace("{{instruction}}", instruction)
        .split_whitespace()
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn builtin_profiles_and_presets_resolve() {
        let reg = CandidateRegistry::load(None, None).unwrap();
        assert!(reg.profile("codex").is_ok());
        assert!(reg.profile("manual").is_ok());
        let strict = reg.preset("strict").unwrap();
        assert!(strict.plan_first && strict.require_full_evidence);
        assert_eq!(
            reg.profile("nope").unwrap_err().kind,
            DraftErrorKind::CandidateNotConfigured
        );
    }

    #[test]
    fn project_config_overrides_global_and_builtin() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global.toml");
        let project = tmp.path().join("project.toml");
        write(
            &global,
            r#"
schema_version = 1

[candidates.codex]
kind = "agent"
command = "codex-global"
can_verify = true

[tasks.presets.fast]
candidates = ["claude"]
"#,
        );
        write(
            &project,
            r#"
schema_version = 1

[candidates.codex]
kind = "agent"
command = "codex-project"
supports_resume = true
max_runtime_seconds = 60
"#,
        );
        let reg = CandidateRegistry::load(Some(&project), Some(&global)).unwrap();
        let codex = reg.profile("codex").unwrap();
        assert_eq!(
            codex.command.as_deref(),
            Some("codex-project {{instruction}}")
        );
        assert!(codex.capabilities.supports_resume);
        // Global-layer capability survives when project does not override it.
        assert!(codex.capabilities.can_verify);
        assert_eq!(codex.limits.max_runtime_seconds, Some(60));
        assert_eq!(reg.preset("fast").unwrap().candidates, vec!["claude"]);
    }

    #[test]
    fn capability_gate_reports_missing_capability() {
        let reg = CandidateRegistry::load(None, None).unwrap();
        let codex = reg.profile("codex").unwrap();
        codex.ensure_capability("edit").unwrap();
        let err = codex.ensure_capability("resume").unwrap_err();
        assert_eq!(err.kind, DraftErrorKind::CandidateNotConfigured);
    }

    #[test]
    fn render_command_splits_instruction() {
        assert_eq!(
            render_command("codex {{instruction}}", "fix the bug"),
            vec!["codex", "fix", "the", "bug"]
        );
    }
}
