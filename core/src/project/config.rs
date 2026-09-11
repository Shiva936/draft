//! Config resolution with strict precedence.
//!
//! See `docs/reference/configuration.md`.
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

use crate::project::Workspace;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

use toml::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HooksConfig {
    #[serde(default)]
    pub verify: Option<HookConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HookConfig {
    Raw { command: String },
    Entry { entry: HookEntry },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
            Self::Raw { command } if !command.trim().is_empty() => Some(HookEntry {
                command: command.clone(),
                enabled: true,
                phase: default_hook_phase(),
                shell: default_hook_shell(),
                cwd: default_hook_cwd(),
                timeout_ms: None,
                continue_on_error: false,
                env: BTreeMap::new(),
            }),
            Self::Raw { .. } => None,
            Self::Entry { entry } if entry.enabled && !entry.command.trim().is_empty() => {
                Some(entry.clone())
            }
            Self::Entry { .. } => None,
        }
    }
}

/// Fully resolved project configuration used by orchestration.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedConfig {
    pub(crate) user_name: String,
    pub(crate) user_email: Option<String>,
    hooks: HooksConfig,
}

impl ResolvedConfig {
    pub(crate) fn load(workspace: &Workspace) -> DraftResult<Self> {
        let global = crate::project::home::DraftGlobalStore::locate()?.config_toml();
        let config = resolve(
            Some(workspace.layout.config_toml().as_path()),
            Some(global.as_path()),
        )?;
        Ok(Self {
            user_name: config.user.name.unwrap_or_else(|| "unknown".to_string()),
            user_email: config.user.email,
            hooks: config.hooks,
        })
    }

    pub(crate) fn hook(&self, name: &str) -> Option<HookEntry> {
        match name {
            "verify" => self.hooks.verify.as_ref().and_then(HookConfig::entry),
            _ => None,
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<String> {
        match key {
            "user.name" => Some(self.user_name.clone()),
            "user.email" => self.user_email.clone(),
            "hooks.verify" => self.hooks.verify.as_ref().map(hook_config_command),
            _ => None,
        }
    }

    pub(crate) fn entries(&self) -> BTreeMap<String, String> {
        ["user.name", "user.email", "hooks.verify"]
            .into_iter()
            .map(|key| (key.to_string(), self.get(key).unwrap_or_default()))
            .collect()
    }
}

fn hook_config_command(hook: &HookConfig) -> String {
    match hook {
        HookConfig::Raw { command } => command.clone(),
        HookConfig::Entry { entry } => entry.command.clone(),
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
#[serde(deny_unknown_fields)]
pub struct VerificationConfig {
    #[serde(default = "default_verification_profile")]
    pub default_profile: String,
}

impl Default for VerificationConfig {
    fn default() -> Self {
        Self {
            default_profile: default_verification_profile(),
        }
    }
}

fn default_verification_profile() -> String {
    "standard".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyConfigSection {
    #[serde(default = "default_true")]
    pub require_verification: bool,
    #[serde(default = "default_true")]
    pub require_approval: bool,
    #[serde(default = "default_true")]
    pub require_human_approval_for_high_risk: bool,
    #[serde(default = "default_true")]
    pub block_on_failed_verification: bool,
}

impl Default for PolicyConfigSection {
    fn default() -> Self {
        Self {
            require_verification: true,
            require_approval: true,
            require_human_approval_for_high_risk: true,
            block_on_failed_verification: true,
        }
    }
}

/// Candidate overrides live in the config contract while resolved candidate
/// profiles remain owned by the task domain.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateConfig {
    pub kind: Option<String>,
    pub command: Option<String>,
    pub can_plan: Option<bool>,
    pub can_edit: Option<bool>,
    pub can_verify: Option<bool>,
    pub can_read_index: Option<bool>,
    pub can_accept_task_contract: Option<bool>,
    pub supports_resume: Option<bool>,
    pub supports_streaming_logs: Option<bool>,
    pub proposes_mutations: Option<bool>,
    pub supports_change_output: Option<bool>,
    pub requires_shell: Option<bool>,
    pub requires_network: Option<bool>,
    pub max_runtime_seconds: Option<u64>,
    pub max_files_changed: Option<u32>,
    pub max_output_bytes_changed: Option<u64>,
    pub max_processes: Option<u32>,
    pub max_output_bytes: Option<u64>,
    pub network: Option<String>,
    pub isolation: Option<String>,
    pub allowed_paths: Option<Vec<String>>,
    pub forbidden_paths: Option<Vec<String>>,
    pub env_allowlist: Option<Vec<String>>,
    pub command_allowlist: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskPresetConfig {
    #[serde(default)]
    pub candidates: Vec<String>,
    #[serde(default)]
    pub plan_first: bool,
    #[serde(default)]
    pub require_full_evidence: bool,
    #[serde(default)]
    pub prefer_smallest_valid_change: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskConfig {
    #[serde(default)]
    pub presets: BTreeMap<String, TaskPresetConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskSettings {
    pub block_on_critical: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportSettings {
    pub require_local_verify: Option<bool>,
    pub max_artifact_bytes: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleSettings {
    pub bind: Option<String>,
    pub port: Option<u16>,
}

/// Protected-path rules embedded in the canonical configuration contract.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedConfig {
    #[serde(default)]
    pub protected_resources: Vec<ProtectedRuleConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedRuleConfig {
    pub pattern: String,
    pub reason: Option<String>,
}

/// Ownership rules embedded in the canonical configuration contract.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OwnershipConfig(BTreeMap<String, Vec<String>>);

impl OwnershipConfig {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Vec<String>)> {
        self.0.iter()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftConfig {
    pub schema_version: u32,
    #[serde(default)]
    pub user: UserConfig,
    #[serde(default)]
    pub hooks: HooksConfig,
    #[serde(default)]
    pub verification: VerificationConfig,
    #[serde(default)]
    pub policy: PolicyConfigSection,
    #[serde(default)]
    pub protected: ProtectedConfig,
    #[serde(default)]
    pub owners: OwnershipConfig,
    #[serde(default)]
    pub candidates: BTreeMap<String, CandidateConfig>,
    #[serde(default)]
    pub tasks: TaskConfig,
    #[serde(default)]
    pub risk: RiskSettings,
    #[serde(default, rename = "import")]
    pub import_settings: ImportSettings,
    #[serde(default)]
    pub console: ConsoleSettings,
}

impl crate::contracts::VersionedContract for DraftConfig {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::DraftConfig;
}

impl Default for DraftConfig {
    fn default() -> Self {
        Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::DraftConfig,
            ),
            user: UserConfig::default(),
            hooks: HooksConfig::default(),
            verification: VerificationConfig::default(),
            policy: PolicyConfigSection::default(),
            protected: ProtectedConfig::default(),
            owners: OwnershipConfig::default(),
            candidates: BTreeMap::new(),
            tasks: TaskConfig::default(),
            risk: RiskSettings::default(),
            import_settings: ImportSettings::default(),
            console: ConsoleSettings::default(),
        }
    }
}

impl DraftConfig {
    pub fn set(&mut self, key: &str, value: &str) -> DraftResult<()> {
        match key {
            "user.name" => self.user.name = Some(validate_profile_value(key, value)?),
            "user.email" => self.user.email = Some(validate_profile_value(key, value)?),
            "hooks.verify" => {
                self.hooks.verify = Some(HookConfig::Raw {
                    command: value.to_string(),
                });
            }
            _ => {
                return Err(DraftError::invalid_config(format!(
                    "unsupported config key '{key}'"
                )));
            }
        }
        Ok(())
    }

    pub fn unset(&mut self, key: &str) -> DraftResult<()> {
        match key {
            "user.name" => self.user.name = None,
            "user.email" => self.user.email = None,
            "hooks.verify" => self.hooks.verify = None,
            _ => {
                return Err(DraftError::invalid_config(format!(
                    "unsupported config key '{key}'"
                )));
            }
        }
        Ok(())
    }
}

pub fn read(path: &Path) -> DraftResult<DraftConfig> {
    if !path.exists() {
        return Ok(DraftConfig::default());
    }
    load_table(Some(path))?;
    fsutil::read_toml(path)
}

/// Resolve canonical config layers without losing whether a field was absent
/// from an overlay. Tables merge recursively; scalar and array values replace
/// the lower-precedence value.
pub fn resolve(
    project_config: Option<&Path>,
    global_config: Option<&Path>,
) -> DraftResult<DraftConfig> {
    let mut value = Value::try_from(DraftConfig::default()).map_err(|error| {
        DraftError::new(
            DraftErrorKind::Internal,
            format!("cannot serialize built-in configuration: {error}"),
        )
    })?;
    for path in [global_config, project_config].into_iter().flatten() {
        if !path.exists() {
            continue;
        }
        // Typed decoding closes the root and nested contract before the raw
        // value participates in layering.
        read(path)?;
        let overlay = load_table(Some(path))?;
        merge_value(&mut value, overlay);
    }
    value.try_into().map_err(|error| {
        DraftError::new(
            DraftErrorKind::CorruptData,
            format!("resolved configuration is invalid: {error}"),
        )
    })
}

fn merge_value(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Table(base), Value::Table(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(&key) {
                    Some(current) => merge_value(current, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

/// Read the ownership and protected-path portions of a configuration through
/// the same strict schema-version policy as every other configuration access.
pub fn rules(path: &Path) -> DraftResult<(ProtectedConfig, OwnershipConfig)> {
    if !path.exists() {
        return Ok((ProtectedConfig::default(), OwnershipConfig::default()));
    }
    load_table(Some(path))?;
    let config = read(path)?;
    Ok((config.protected, config.owners))
}

/// Canonical digest of parsed project configuration. Formatting and comments
/// are intentionally excluded.
pub fn config_hash(paths: &crate::project::layout::DraftLayout) -> DraftResult<String> {
    let read = |path: &Path| -> DraftResult<serde_json::Value> {
        if !path.exists() {
            return Ok(serde_json::Value::Null);
        }
        let value = load_table(Some(path))?;
        serde_json::to_value(value).map_err(|error| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "cannot canonicalize configuration {}: {error}",
                    path.display()
                ),
            )
        })
    };
    Ok(crate::support::hashing::canonical_hash(
        &serde_json::json!({
            "config": read(&paths.config_toml())?,
            "policy": read(&paths.policy_toml())?,
        }),
    ))
}

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
    pub fn load(project_config: Option<&Path>, global_config: Option<&Path>) -> DraftResult<Self> {
        Ok(ConfigResolver {
            cli: BTreeMap::new(),
            project: load_table(project_config)?,
            global: load_table(global_config)?,
            defaults: builtin_defaults(),
        })
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
        if key == "user.name" {
            return Some("unknown".to_string());
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
        if key == "user.name" {
            return Some("fallback");
        }
        None
    }
}

/// Write `key = value` into the config file for `scope`, creating it if needed.
pub fn set_value(path: &Path, key: &str, value: &str) -> DraftResult<()> {
    let mut table = load_table(Some(path))?;
    set_dotted(
        &mut table,
        "schema_version",
        Value::Integer(
            crate::contracts::current_version(crate::contracts::ContractId::DraftConfig).into(),
        ),
    );
    let value = encoded_config_value(key, value)?;
    set_dotted(&mut table, key, value);
    fsutil::write_toml(path, &table)
}

/// Remove one canonical configuration table. Missing tables are idempotent.
pub fn remove_table(path: &Path, key: &str) -> DraftResult<()> {
    let mut table = load_table(Some(path))?;
    set_dotted(
        &mut table,
        "schema_version",
        Value::Integer(
            crate::contracts::current_version(crate::contracts::ContractId::DraftConfig).into(),
        ),
    );
    remove_dotted(&mut table, key);
    fsutil::write_toml(path, &table)
}

/// Atomically apply multiple config-layer updates. `None` removes a key; a
/// value is validated and written without materializing inherited defaults.
pub fn update_values(path: &Path, updates: &[(String, Option<String>)]) -> DraftResult<()> {
    let mut table = load_table(Some(path))?;
    set_dotted(
        &mut table,
        "schema_version",
        Value::Integer(
            crate::contracts::current_version(crate::contracts::ContractId::DraftConfig).into(),
        ),
    );
    for (key, value) in updates {
        if key == "identity" || key.starts_with("identity.") {
            return Err(retired_profile_error("configuration key", key));
        }
        match value {
            Some(value) => set_dotted(&mut table, key, encoded_config_value(key, value)?),
            None => remove_dotted(&mut table, key),
        }
    }
    fsutil::write_toml(path, &table)
}

/// Encode a supported dotted-key value in the same representation that the
/// typed `DraftConfig` decoder expects. Structured hook contracts must not be
/// flattened to scalar TOML merely because the CLI accepts a command string.
fn encoded_config_value(key: &str, value: &str) -> DraftResult<Value> {
    let structured = match key {
        "hooks.verify" => Some(Value::try_from(HookConfig::Raw {
            command: value.to_string(),
        })),
        _ => None,
    };
    if let Some(value) = structured {
        return value.map_err(|error| {
            DraftError::invalid_config(format!("cannot encode configuration key '{key}': {error}"))
        });
    }
    match key {
        "user.name" | "user.email" => Ok(Value::String(validate_profile_value(key, value)?)),
        key if key == "identity" || key.starts_with("identity.") => {
            Err(retired_profile_error("configuration key", key))
        }
        _ => Ok(parse_scalar(value)),
    }
}

/// Read `key` from a single config file (used by `config get` when a scope is
/// pinned). Returns `None` if absent.
pub fn get_value(path: &Path, key: &str) -> DraftResult<Option<String>> {
    Ok(get_dotted(&load_table(Some(path))?, key).map(scalar_to_string))
}

// ---- Built-in defaults ---------------------------------------------------

fn builtin_defaults() -> Value {
    // Kept intentionally small; each key has a safe, offline value.
    let toml = r#"
schema_version = 1

[risk]
block_on_critical = true

[import]
require_local_verify = true
max_artifact_bytes = 104857600

[console]
bind = "127.0.0.1"
port = 4317
"#;
    toml::from_str(toml).expect("built-in defaults must parse")
}

const USER_NAME_MAX_BYTES: usize = 256;
const USER_EMAIL_MAX_BYTES: usize = 512;

/// Validate non-authoritative profile/display metadata without pretending to
/// validate mailbox ownership or deliverability.
pub fn validate_profile_value(key: &str, value: &str) -> DraftResult<String> {
    let value = value.trim();
    let limit = match key {
        "user.name" => USER_NAME_MAX_BYTES,
        "user.email" => USER_EMAIL_MAX_BYTES,
        _ => {
            return Err(DraftError::invalid_config(format!(
                "unsupported profile key '{key}'"
            )))
        }
    };
    if value.is_empty() {
        return Err(DraftError::invalid_config(format!("{key} cannot be empty")));
    }
    if value.len() > limit {
        return Err(DraftError::invalid_config(format!(
            "{key} exceeds the {limit}-byte limit"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(DraftError::invalid_config(format!(
            "{key} cannot contain control characters"
        )));
    }
    Ok(value.to_string())
}

fn retired_profile_error(kind: &str, location: impl std::fmt::Display) -> DraftError {
    DraftError::new(
        DraftErrorKind::UnsupportedSchema,
        format!("unsupported pre-release profile {kind}: {location}"),
    )
    .with_suggestion(
        "remove the retired identity profile state; configure display metadata with `draft config set user.name ...` and optional `user.email`",
    )
}

/// Detect the retired namespace structurally without deserializing or applying
/// any of its values. This runs before the TOML document enters layering.
fn reject_retired_profile_source(source: &str, path: &Path) -> DraftResult<()> {
    for raw_line in source.lines() {
        let line = raw_line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let structural = line.split('#').next().unwrap_or(line).trim();
        let retired_table = structural
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
            .map(str::trim)
            .is_some_and(|table| table == "identity" || table.starts_with("identity."));
        let retired_key = structural
            .split_once('=')
            .map(|(key, _)| key.trim().trim_matches('"').trim_matches('\''))
            .is_some_and(|key| key == "identity" || key.starts_with("identity."));
        if retired_table || retired_key {
            return Err(retired_profile_error(
                "configuration namespace in",
                path.display(),
            ));
        }
    }
    Ok(())
}

/// Preflight one config document for the retired namespace without building a
/// typed config or applying any value from it.
pub fn reject_retired_profile_config(path: &Path) -> DraftResult<()> {
    if !path.exists() {
        return Ok(());
    }
    let source = std::fs::read_to_string(path)?;
    reject_retired_profile_source(&source, path)
}

// ---- Dotted-key helpers over toml::Value ---------------------------------

fn load_table(path: Option<&Path>) -> DraftResult<Value> {
    match path {
        Some(path) if path.exists() => {
            let source = std::fs::read_to_string(path)?;
            reject_retired_profile_source(&source, path)?;
            let value: Value = toml::from_str(&source).map_err(|error| {
                crate::support::error::DraftError::new(
                    crate::support::error::DraftErrorKind::CorruptData,
                    format!("malformed configuration {}: {error}", path.display()),
                )
            })?;
            match get_dotted(&value, "schema_version") {
                Some(Value::Integer(version))
                    if u32::try_from(*version).is_ok_and(|version| {
                        crate::contracts::supports_version(
                            crate::contracts::ContractId::DraftConfig,
                            version,
                        )
                    }) => {}
                Some(Value::Integer(version)) => {
                    return Err(crate::support::error::DraftError::new(
                        crate::support::error::DraftErrorKind::UnsupportedSchema,
                        format!("unsupported configuration schema {version}"),
                    ));
                }
                _ => {
                    return Err(crate::support::error::DraftError::new(
                        crate::support::error::DraftErrorKind::CorruptData,
                        format!(
                            "configuration requires numeric schema_version = {}",
                            crate::contracts::current_version(
                                crate::contracts::ContractId::DraftConfig
                            )
                        ),
                    ));
                }
            }
            Ok(value)
        }
        _ => Ok(empty_table()),
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

fn remove_dotted(root: &mut Value, key: &str) {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.is_empty() {
        return;
    }
    let mut cur = root;
    for part in &parts[..parts.len() - 1] {
        let Some(next) = cur.as_table_mut().and_then(|table| table.get_mut(*part)) else {
            return;
        };
        cur = next;
    }
    if let Some(table) = cur.as_table_mut() {
        table.remove(parts[parts.len() - 1]);
    }
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

        let r = ConfigResolver::load(Some(&proj), Some(&glob)).unwrap();
        assert_eq!(r.get("risk.block_on_critical").as_deref(), Some("true"));
        assert_eq!(r.source_of("risk.block_on_critical"), Some("project"));

        let r = r.with_cli_override("risk.block_on_critical", "false");
        assert_eq!(r.get("risk.block_on_critical").as_deref(), Some("false"));
        assert_eq!(r.source_of("risk.block_on_critical"), Some("cli"));
    }

    #[test]
    fn falls_back_to_builtin_default() {
        let r = ConfigResolver::load(None, None).unwrap();
        assert_eq!(r.get("schema_version").as_deref(), Some("1"));
        assert_eq!(r.source_of("schema_version"), Some("default"));
        assert_eq!(r.get("console.port").as_deref(), Some("4317"));
    }

    #[test]
    fn set_then_get_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("config.toml");
        set_value(&proj, "custom.key", "hello").unwrap();
        assert_eq!(
            get_value(&proj, "custom.key").unwrap().as_deref(),
            Some("hello")
        );
    }

    #[test]
    fn retired_identity_namespace_is_rejected_before_its_value_is_parsed() {
        let temp = tempfile::tempdir().unwrap();
        for source in [
            "schema_version = 1\n[identity]\nsecret = [not valid TOML\n",
            "schema_version = 1\nidentity.email = [not valid TOML\n",
        ] {
            let path = temp.path().join(format!("{}.toml", uuid::Uuid::new_v4()));
            std::fs::write(&path, source).unwrap();
            let error = read(&path).unwrap_err();
            assert_eq!(error.kind, DraftErrorKind::UnsupportedSchema);
            assert!(error.message.contains("unsupported pre-release profile"));
        }
    }

    #[test]
    fn user_profile_validation_is_modest_and_fallback_is_never_persisted() {
        assert_eq!(
            validate_profile_value("user.email", "  Display contact, not a mailbox  ").unwrap(),
            "Display contact, not a mailbox"
        );
        for value in ["", "  ", "line\nfeed"] {
            assert!(validate_profile_value("user.name", value).is_err());
            assert!(validate_profile_value("user.email", value).is_err());
        }
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let resolver = ConfigResolver::load(None, Some(&path)).unwrap();
        assert_eq!(resolver.get("user.name").as_deref(), Some("unknown"));
        assert_eq!(resolver.source_of("user.name"), Some("fallback"));
        assert!(!path.exists());
        set_value(&path, "user.email", "person@example.test").unwrap();
        remove_table(&path, "user.email").unwrap();
        let source = std::fs::read_to_string(path).unwrap();
        assert!(!source.contains("unknown"));
        assert!(!source.contains("email"));
    }
}
