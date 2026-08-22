mod output;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use draft_core::error::{DraftError, DraftErrorKind};
use draft_core::App;

#[derive(Parser)]
#[command(name = "draft", version = draft_core::DRAFT_VERSION, about = "Draft v0.3.4 - Task-centered verified change review")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(flatten)]
    Workspace(WorkspaceCommand),
    #[command(flatten)]
    Tasks(TaskCommand),
    #[command(flatten)]
    Packs(PackCommand),
    #[command(flatten)]
    Review(ReviewCommand),
    #[command(flatten)]
    Integration(IntegrationCommand),
    #[command(flatten)]
    Maintenance(MaintenanceCommand),
}

#[derive(Subcommand)]
enum WorkspaceCommand {
    /// Initialize a Draft workspace (or the global store with --global).
    Init {
        #[arg(short = 'b')]
        base: Option<String>,
        /// Initialize the global `~/.draft/` store instead of a project.
        #[arg(long)]
        global: bool,
        #[arg(long)]
        json: bool,
    },
    /// Validate global and project Draft state.
    Doctor {
        #[command(subcommand)]
        action: Option<DoctorAction>,
        /// Validate only the global store.
        #[arg(long)]
        global: bool,
        #[arg(long)]
        json: bool,
    },
    /// Inspect Draft identity.
    Identity {
        #[command(subcommand)]
        action: IdentityAction,
    },
    /// Launch the local Draft Console in a browser.
    Console {
        /// Port to bind (loopback only).
        #[arg(long, default_value_t = 4317)]
        port: u16,
    },
    /// Manage Draft extensions.
    Extension {
        #[command(subcommand)]
        action: ExtensionAction,
    },
    /// Manage workspace config.
    Config {
        #[arg(short = 'k')]
        key: Option<String>,
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
    /// Manage hooks.
    Hook {
        #[arg(short = 'k')]
        key: Option<String>,
        #[command(subcommand)]
        action: Option<HookAction>,
    },
    /// Manage .draft/.ignore.
    Ignore {
        #[command(subcommand)]
        action: IgnoreAction,
    },
    /// Show Draft-native workspace status.
    Status {
        #[arg(short = 'p')]
        pack: Option<String>,
        #[arg(short = 'c')]
        component: Option<String>,
        #[arg(long)]
        full: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show append-only events.
    Event {
        #[arg(long)]
        page: Option<usize>,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        raw: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum TaskCommand {
    /// Manage tasks.
    Task {
        #[command(subcommand)]
        action: Option<TaskAction>,
    },
    /// Show items requiring attention and their next safe action.
    Inbox {
        #[arg(long)]
        json: bool,
    },
    /// Create an audited, expiring waiver for a pack finding.
    Waive {
        pack_id: String,
        finding_id: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        expires: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum PackCommand {
    /// Create a Draft-native checkpoint.
    Checkpoint {
        message: String,
        #[arg(long)]
        json: bool,
    },
    /// Create a ChangePack.
    Create {
        name: String,
        #[arg(short = 'p')]
        base_pack: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Inspect/compose packs, or switch/delete/export/import a ChangePack.
    Pack {
        /// Pack algebra subcommand (inspect/depends/conflicts/compose).
        #[command(subcommand)]
        algebra: Option<PackAlgebra>,
        #[arg(short = 's')]
        select: Option<String>,
        #[arg(short = 'd')]
        delete: Option<String>,
        /// Export a pack (by pck_id or name) to a portable .draftpack.
        #[arg(long)]
        export: Option<String>,
        /// Import a .draftpack artifact into quarantine.
        #[arg(long, value_name = "path")]
        import: Option<String>,
        /// Output path for --export.
        #[arg(long)]
        output: Option<String>,
        /// Assign a new unique workspace-local name on --import.
        #[arg(long)]
        name: Option<String>,
        /// With --import, validate and report without mutating state.
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// List available ChangePacks.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Manage candidates.
    Candidate {
        #[command(subcommand)]
        action: CandidateAction,
    },
}

#[derive(Subcommand)]
enum ReviewCommand {
    /// Verify a pack: risk + evidence-based test/fuzz selection.
    Verify {
        /// Pack to verify (pck_id or name); enables v0.3.3 evidence verification.
        target: Option<String>,
        #[arg(short = 'p')]
        pack: Option<String>,
        /// Show why tests and fuzz targets were selected.
        #[arg(long)]
        explain: bool,
        /// Select the full configured test suite.
        #[arg(long)]
        full: bool,
        /// Include selected fuzz targets.
        #[arg(long)]
        fuzz: bool,
        #[arg(long)]
        json: bool,
    },
    /// Assess risk.
    Risk {
        #[arg(short = 'p')]
        pack: Option<String>,
        #[arg(long)]
        explain: bool,
        #[arg(long)]
        include_evidence: bool,
        #[arg(long)]
        json: bool,
    },
    /// Review a changepack or launch the TUI.
    Review {
        #[arg(short = 'p')]
        pack: Option<String>,
        #[arg(long)]
        tui: bool,
        #[arg(long)]
        comment: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Approve a changepack.
    Approve {
        #[arg(short = 'p')]
        pack: Option<String>,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Reject a changepack.
    Reject {
        #[arg(short = 'p')]
        pack: Option<String>,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum IntegrationCommand {
    /// Compare changepacks.
    Compare {
        left: String,
        right: String,
        #[arg(long)]
        tui: bool,
        #[arg(long)]
        json: bool,
    },
    /// Compose non-overlapping changepacks.
    Compose {
        left: String,
        right: String,
        #[arg(long)]
        output: String,
        #[arg(long)]
        tui: bool,
        #[arg(long)]
        json: bool,
    },
    /// Split a ChangePack into two output ChangePacks.
    Disperse {
        pack: String,
        #[arg(long, num_args = 2)]
        output: Vec<String>,
        #[arg(long)]
        tui: bool,
        #[arg(long)]
        json: bool,
    },
    /// Submit an approved changepack into stable_head and optionally run hooks.submit.
    Submit {
        #[arg(short = 'p')]
        pack: Option<String>,
        #[arg(long = "var", num_args = 1.., allow_hyphen_values = true, value_name = "key=value")]
        vars: Vec<String>,
        /// Show what would be submitted and which checks pass, without submitting.
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Roll back to a checkpoint, pack base snapshot, or receipt.
    Rollback {
        reference: String,
        /// Resolve the target and report the plan without mutating anything.
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Inspect receipts.
    Receipt {
        #[command(subcommand)]
        action: ReceiptAction,
    },
}

#[derive(Subcommand)]
enum MaintenanceCommand {
    /// Remove Draft metadata from this workspace.
    Close {
        #[arg(long)]
        force: bool,
    },
    /// Run safe Draft metadata maintenance.
    Gc,
    /// Manage storage.
    Storage {
        #[command(subcommand)]
        action: StorageAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Read a config value using CLI > project > global > default precedence.
    Get {
        key: String,
        /// Read only from the global `~/.draft/config.toml` layer.
        #[arg(long)]
        global: bool,
        #[arg(long)]
        json: bool,
    },
    Set {
        key: String,
        value: String,
        /// Write to the global `~/.draft/config.toml` instead of the project.
        #[arg(long)]
        global: bool,
        #[arg(long)]
        json: bool,
    },
    Unset {
        key: String,
        /// Clear the value in the global `~/.draft/config.toml` layer.
        #[arg(long)]
        global: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum DoctorAction {
    Sync {
        #[arg(long)]
        fix: bool,
        #[arg(long)]
        json: bool,
    },
    Stats {
        #[arg(long)]
        json: bool,
    },
    Gc {
        #[arg(long)]
        json: bool,
    },
    Compact {
        #[arg(long)]
        json: bool,
    },
    Prune {
        #[arg(long)]
        json: bool,
    },
    Index {
        #[arg(long)]
        refresh: bool,
        #[arg(long)]
        global: bool,
        #[arg(long)]
        json: bool,
    },
    Migrate {
        #[arg(long)]
        check: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum PackAlgebra {
    /// Inspect a pack: manifest, state, risk, evidence, receipts, impact.
    Inspect {
        pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Compute a pack's dependencies and shared-symbol relationships.
    Depends {
        pack_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Detect conflicts between two packs.
    Conflicts {
        pack_a: String,
        pack_b: String,
        #[arg(long)]
        json: bool,
    },
    /// Compose two packs into a new (unverified) pack.
    Compose {
        pack_a: String,
        pack_b: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum IdentityAction {
    /// Show the active actor and signing-key availability.
    Status {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ExtensionAction {
    List {
        #[arg(long)]
        json: bool,
    },
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    Install {
        path: String,
        #[arg(long)]
        json: bool,
    },
    Uninstall {
        id: String,
        #[arg(long)]
        json: bool,
    },
    Enable {
        id: String,
        #[arg(long)]
        json: bool,
    },
    Disable {
        id: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum HookAction {
    Set {
        key: String,
        value: String,
        #[arg(long)]
        json: bool,
    },
    Unset {
        key: String,
        #[arg(long)]
        json: bool,
    },
    Run {
        hook_name: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum IgnoreAction {
    Add {
        pattern: String,
        #[arg(long)]
        json: bool,
    },
    Remove {
        pattern: String,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum TaskAction {
    #[command(flatten)]
    Definition(TaskDefinitionAction),
    #[command(flatten)]
    Execution(TaskExecutionAction),
    #[command(external_subcommand)]
    External(Vec<String>),
}

#[derive(Subcommand)]
enum TaskDefinitionAction {
    /// Create a stored deterministic task definition.
    Create {
        name: String,
        #[arg(long)]
        goal: String,
        #[arg(long)]
        template: Option<String>,
        #[arg(long = "allow")]
        allowed_zones: Vec<String>,
        #[arg(long = "forbid")]
        forbidden_zones: Vec<String>,
        #[arg(long = "success")]
        success_criteria: Vec<String>,
        #[arg(long = "candidate-preset")]
        candidate_preset: Option<String>,
        #[arg(long, value_parser = ["low", "medium", "high", "critical"])]
        risk: Option<String>,
        #[arg(long, value_parser = ["normal", "safe", "plan-first"])]
        mode: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Create a task through deterministic stdin prompts.
    Wizard {
        #[arg(long)]
        json: bool,
    },
    /// Clear task runtime state, or remove the definition too with --hard.
    Drop {
        task: String,
        #[arg(long)]
        hard: bool,
        #[arg(long)]
        json: bool,
    },
    Export {
        task: String,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Import {
        path: PathBuf,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    /// Inspect a task by id or name.
    Show {
        task: String,
        #[arg(long)]
        full: bool,
        #[arg(long)]
        executions: bool,
        #[arg(long)]
        packs: bool,
        #[arg(long)]
        conflicts: bool,
        #[arg(long)]
        lanes: bool,
        #[arg(long)]
        evidence: bool,
        #[arg(long)]
        timeline: bool,
        #[arg(long)]
        explain: bool,
        #[arg(long)]
        decompose: bool,
        #[arg(long = "diff-stable")]
        diff_stable: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum TaskExecutionAction {
    Spawn {
        name: String,
        #[arg(short = 'p')]
        pack: Option<String>,
        #[arg(short = 'c')]
        candidates: Vec<String>,
        #[arg(long)]
        preset: Option<String>,
        #[arg(long)]
        resume: Option<String>,
        #[arg(long)]
        cancel: Option<String>,
        #[arg(long)]
        retry: Option<String>,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        cron: Option<String>,
        #[arg(last = true)]
        instruction: Vec<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum CandidateAction {
    List {
        #[arg(long)]
        json: bool,
    },
    Show {
        candidate_name: String,
        #[arg(long)]
        json: bool,
    },
    Add(CandidateMutationArgs),
    Update(CandidateMutationArgs),
    Remove {
        candidate_name: String,
        #[arg(long)]
        json: bool,
    },
    Packs {
        #[arg(short = 'p')]
        pack: Option<String>,
        #[arg(short = 'c')]
        candidate: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct CandidateMutationArgs {
    candidate_name: String,
    #[arg(long, value_parser = ["command", "chat", "manual"])]
    kind: Option<String>,
    #[arg(last = true, required = true)]
    template: Vec<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum ReceiptAction {
    List {
        #[arg(long)]
        json: bool,
    },
    Show {
        receipt_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Verify a signed receipt (or all with --all).
    Verify {
        /// Receipt id (rcp_...). Omit with --all to verify everything.
        receipt_id: Option<String>,
        /// Verify the event chain, transparency chain, and every receipt.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum StorageAction {
    Stats {
        #[arg(long)]
        json: bool,
    },
    Gc {
        #[arg(long)]
        json: bool,
    },
    Compact {
        #[arg(long)]
        json: bool,
    },
    Prune {
        #[arg(long)]
        json: bool,
    },
    Doctor {
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", output::format_error(&e));
            match e.kind {
                DraftErrorKind::VerificationFailed => ExitCode::from(5),
                DraftErrorKind::RiskPolicyBlocked => ExitCode::from(6),
                DraftErrorKind::ReviewRequired => ExitCode::from(7),
                DraftErrorKind::SubmitFailed | DraftErrorKind::SubmitReadinessBlocked => {
                    ExitCode::from(8)
                }
                DraftErrorKind::Storage => ExitCode::from(9),
                DraftErrorKind::ConflictDetected => ExitCode::from(2),
                _ => ExitCode::FAILURE,
            }
        }
    }
}

fn run(cli: Cli) -> Result<(), DraftError> {
    let cwd = std::env::current_dir().map_err(DraftError::from)?;
    ensure_project_scope(&cli.command, cwd.as_path())?;
    let app = App::new();
    match cli.command {
        Command::Workspace(command) => run_workspace(&app, cwd.as_path(), command),
        Command::Tasks(command) => run_tasks(&app, cwd.as_path(), command),
        Command::Packs(command) => run_packs(&app, cwd.as_path(), command),
        Command::Review(command) => run_review(&app, cwd.as_path(), command),
        Command::Integration(command) => run_integration(&app, cwd.as_path(), command),
        Command::Maintenance(command) => run_maintenance(&app, cwd.as_path(), command),
    }
}

fn run_workspace(app: &App, cwd: &Path, command: WorkspaceCommand) -> Result<(), DraftError> {
    match command {
        WorkspaceCommand::Init { base, global, json } => {
            if global {
                render_init_global(app.init_global()?, json)
            } else {
                render_init(
                    app.init_with_base(cwd, base.as_deref().unwrap_or("base"))?,
                    json,
                )
            }
        }
        WorkspaceCommand::Doctor {
            action: Some(DoctorAction::Sync { fix, json }),
            ..
        } => {
            let registry = draft_core::registry::ProjectRegistry::global()?;
            let report = if fix {
                serde_json::to_value(registry.fix_stale()?)
            } else {
                serde_json::to_value(registry.inspect()?)
            }
            .map_err(|e| DraftError::storage(e.to_string()))?;
            render_json_or_text(
                report,
                json,
                if fix {
                    "Registry synchronized"
                } else {
                    "Registry status"
                },
            )
        }
        WorkspaceCommand::Doctor {
            action: Some(DoctorAction::Stats { json }),
            ..
        } => render_json_or_text(app.storage_stats(cwd)?, json, "Storage statistics"),
        WorkspaceCommand::Doctor {
            action: Some(DoctorAction::Gc { json }),
            ..
        } => render_json_or_text(app.gc(cwd)?, json, "Doctor GC complete"),
        WorkspaceCommand::Doctor {
            action: Some(DoctorAction::Compact { json }),
            ..
        } => render_json_or_text(app.storage_compact(cwd)?, json, "Doctor compact complete"),
        WorkspaceCommand::Doctor {
            action: Some(DoctorAction::Prune { json }),
            ..
        } => render_json_or_text(app.storage_prune(cwd)?, json, "Doctor prune complete"),
        WorkspaceCommand::Doctor {
            action:
                Some(DoctorAction::Index {
                    refresh,
                    global,
                    json,
                }),
            ..
        } => {
            let report = if global {
                app.doctor_index_global(refresh)?
            } else {
                app.doctor_index(cwd, refresh)?
            };
            render_json_or_text(report, json, "Index status")
        }
        WorkspaceCommand::Doctor {
            action: Some(DoctorAction::Migrate { check, json }),
            ..
        } => render_json_or_text(app.doctor_migrate(cwd, check)?, json, "Migration status"),
        WorkspaceCommand::Doctor {
            action: None,
            global,
            json,
        } => {
            let report = if global {
                app.doctor_global()?
            } else {
                app.doctor(cwd)?
            };
            render_doctor(report, json)
        }
        WorkspaceCommand::Identity { action } => match action {
            IdentityAction::Status { json } => render_identity(app.identity_status()?, json),
        },
        WorkspaceCommand::Console { port } => {
            // Ensure we are inside a workspace before starting the server.
            app.status(cwd)?;
            draft_agui::serve(cwd.to_path_buf(), "127.0.0.1", port)
                .map_err(|e| DraftError::new(DraftErrorKind::Internal, e))
        }
        WorkspaceCommand::Extension { action } => match action {
            ExtensionAction::List { json } => {
                render_json_or_text(draft_adapters::extension::list()?, json, "Extensions")
            }
            ExtensionAction::Show { id, json } => {
                render_json_or_text(draft_adapters::extension::show(&id)?, json, "Extension")
            }
            ExtensionAction::Install { path, json } => render_json_or_text(
                draft_adapters::extension::install(Path::new(&path))?,
                json,
                "Extension installed",
            ),
            ExtensionAction::Uninstall { id, json } => render_json_or_text(
                draft_adapters::extension::uninstall(&id)?,
                json,
                "Extension uninstalled",
            ),
            ExtensionAction::Enable { id, json } => render_json_or_text(
                draft_adapters::extension::set_enabled(&id, true)?,
                json,
                "Extension enabled",
            ),
            ExtensionAction::Disable { id, json } => render_json_or_text(
                draft_adapters::extension::set_enabled(&id, false)?,
                json,
                "Extension disabled",
            ),
        },
        WorkspaceCommand::Config { key, action } => match action {
            Some(ConfigAction::Get { key, global, json }) => {
                let report = if global {
                    app.config_get_global(&key)?
                } else {
                    app.config_get_layered(cwd, &key)?
                };
                render_config(report, json)
            }
            Some(ConfigAction::Set {
                key,
                value,
                global,
                json,
            }) => {
                let report = if global {
                    app.config_set_global(&key, &value)?
                } else {
                    app.config_set(cwd, &key, &value)?
                };
                render_config(report, json)
            }
            Some(ConfigAction::Unset { key, global, json }) => {
                let report = if global {
                    app.config_unset_global(&key)?
                } else {
                    app.config_unset(cwd, &key)?
                };
                render_config(report, json)
            }
            None => {
                if let Some(key) = key {
                    render_config(app.config_get(cwd, &key)?, false)
                } else {
                    render_config(app.config_list(cwd)?, false)
                }
            }
        },
        WorkspaceCommand::Hook { key, action } => match action {
            Some(HookAction::Set { key, value, json }) => {
                render_config(app.hook_set(cwd, &key, &value)?, json)
            }
            Some(HookAction::Unset { key, json }) => {
                render_config(app.hook_unset(cwd, &key)?, json)
            }
            Some(HookAction::Run { hook_name, json }) => {
                render_json_or_text(app.hook_run(cwd, &hook_name)?, json, "Hook complete")
            }
            None => {
                if let Some(key) = key {
                    render_config(app.hook_get(cwd, &key)?, false)
                } else {
                    render_config(app.hook_list(cwd)?, false)
                }
            }
        },
        WorkspaceCommand::Ignore { action } => match action {
            IgnoreAction::Add { pattern, json } => {
                render_ignore(app.ignore_add(cwd, &pattern)?, json)
            }
            IgnoreAction::Remove { pattern, json } => {
                render_ignore(app.ignore_remove(cwd, &pattern)?, json)
            }
            IgnoreAction::List { json } => render_ignore(app.ignore_list(cwd)?, json),
        },
        WorkspaceCommand::Status {
            pack,
            component,
            full,
            json,
        } => {
            if pack.is_some() || component.is_some() || full {
                let component = component
                    .as_deref()
                    .map(draft_core::StatusComponent::parse)
                    .transpose()?;
                render_status_report(
                    app.status_with_options(
                        cwd,
                        draft_core::StatusOptions {
                            pack,
                            component,
                            full,
                        },
                    )?,
                    json,
                )
            } else {
                render_status(app.status(cwd)?, json)
            }
        }
        WorkspaceCommand::Event {
            page,
            limit,
            raw,
            json,
        } => render_events(
            app.events_page(cwd, false, false, page, limit, None)?,
            json,
            raw,
        ),
    }
}

fn run_tasks(app: &App, cwd: &Path, command: TaskCommand) -> Result<(), DraftError> {
    match command {
        TaskCommand::Task { action } => match action {
            Some(TaskAction::Definition(TaskDefinitionAction::Create {
                name,
                goal,
                template,
                allowed_zones,
                forbidden_zones,
                success_criteria,
                candidate_preset,
                risk,
                mode,
                json,
            })) => render_json_or_text(
                app.task_define(
                    cwd,
                    &name,
                    &goal,
                    template,
                    allowed_zones,
                    forbidden_zones,
                    success_criteria,
                    risk.as_deref(),
                    mode.as_deref(),
                    candidate_preset,
                )?,
                json,
                "Task created",
            ),
            Some(TaskAction::Execution(TaskExecutionAction::Spawn {
                name,
                pack,
                candidates,
                preset,
                resume,
                cancel,
                retry,
                reason,
                cron,
                instruction,
                json,
            })) => {
                if let Some(execution_id) = cancel {
                    return render_json_or_text(
                        app.task_cancel_execution(cwd, &execution_id, reason)?,
                        json,
                        "Execution cancelled",
                    );
                }
                if let Some(execution_id) = resume {
                    return render_json_or_text(
                        app.task_resume_execution(cwd, &execution_id)?,
                        json,
                        "Execution resume queued",
                    );
                }
                if let Some(execution_id) = retry {
                    return render_json_or_text(
                        app.task_retry_execution(cwd, &execution_id)?,
                        json,
                        "Execution retry queued",
                    );
                }
                render_json_or_text(
                    app.task_spawn_with_preset(
                        cwd,
                        &name,
                        pack.as_deref(),
                        candidates,
                        preset,
                        cron,
                        instruction,
                    )?,
                    json,
                    "Task spawned",
                )
            }
            Some(TaskAction::Definition(TaskDefinitionAction::Wizard { json })) => {
                let task = run_task_wizard(app, cwd, json)?;
                render_json_or_text(task, json, "Task created")
            }
            Some(TaskAction::Definition(TaskDefinitionAction::Drop { task, hard, json })) => {
                render_json_or_text(app.task_drop(cwd, &task, hard)?, json, "Task dropped")
            }
            Some(TaskAction::Definition(TaskDefinitionAction::Export { task, output, json })) => {
                render_json_or_text(
                    app.task_export(cwd, &task, output.as_deref())?,
                    json,
                    "Task exported",
                )
            }
            Some(TaskAction::Definition(TaskDefinitionAction::Import { path, name, json })) => {
                render_json_or_text(app.task_import(cwd, &path, name)?, json, "Task imported")
            }
            Some(TaskAction::Definition(TaskDefinitionAction::List { json })) => {
                render_json_or_text(app.task_definitions(cwd)?, json, "Tasks")
            }
            Some(TaskAction::Definition(TaskDefinitionAction::Show {
                task,
                full,
                executions,
                packs,
                conflicts,
                lanes,
                evidence,
                timeline,
                explain,
                decompose,
                diff_stable,
                json,
            })) => {
                let options = draft_core::TaskViewOptions {
                    full,
                    executions,
                    packs,
                    conflicts,
                    lanes,
                    evidence,
                    timeline,
                    explain,
                    decompose,
                    diff_stable,
                };
                if full
                    || executions
                    || packs
                    || conflicts
                    || lanes
                    || evidence
                    || timeline
                    || explain
                    || decompose
                    || diff_stable
                {
                    render_json_or_text(
                        app.task_view_with_options(cwd, &task, options)?,
                        json,
                        "Task",
                    )
                } else {
                    render_json_or_text(app.task_view(cwd, &task)?, json, "Task")
                }
            }
            Some(TaskAction::External(args)) => {
                let task_id = args
                    .first()
                    .ok_or_else(|| DraftError::invalid_config("missing task id"))?;
                render_json_or_text(app.task_view(cwd, task_id)?, false, "Task")
            }
            None => render_json_or_text(app.task_current(cwd)?, false, "Task"),
        },
        TaskCommand::Inbox { json } => render_json_or_text(app.inbox(cwd)?, json, "Inbox"),
        TaskCommand::Waive {
            pack_id,
            finding_id,
            reason,
            expires,
            json,
        } => render_json_or_text(
            app.waive(cwd, &pack_id, &finding_id, &reason, &expires)?,
            json,
            "Waiver created",
        ),
    }
}

fn run_packs(app: &App, cwd: &Path, command: PackCommand) -> Result<(), DraftError> {
    match command {
        PackCommand::Checkpoint { message, json } => {
            render_json_or_text(app.checkpoint(cwd, &message)?, json, "Checkpoint created")
        }
        PackCommand::Create {
            name,
            base_pack,
            json,
        } => render_json_or_text(
            app.pack_create_from_base(cwd, name, base_pack)?,
            json,
            "ChangePack created",
        ),
        PackCommand::Pack {
            algebra: Some(action),
            ..
        } => match action {
            PackAlgebra::Inspect { pack_id, json } => {
                render_json_or_text(app.pack_inspect(cwd, &pack_id)?, json, "Pack")
            }
            PackAlgebra::Depends { pack_id, json } => {
                render_json_or_text(app.pack_depends(cwd, &pack_id)?, json, "Dependencies")
            }
            PackAlgebra::Conflicts {
                pack_a,
                pack_b,
                json,
            } => {
                let report = app.pack_conflicts(cwd, &pack_a, &pack_b)?;
                let blocking = report.blocking;
                render_json_or_text(report, json, "Conflicts")?;
                if blocking {
                    return Err(DraftError::new(
                        DraftErrorKind::ConflictDetected,
                        "blocking conflicts detected",
                    ));
                }
                Ok(())
            }
            PackAlgebra::Compose {
                pack_a,
                pack_b,
                name,
                json,
            } => render_json_or_text(
                app.pack_compose(cwd, &pack_a, &pack_b, &name)?,
                json,
                "Pack composed (re-verify required)",
            ),
        },
        PackCommand::Pack {
            algebra: None,
            select,
            delete,
            export,
            import,
            output,
            name,
            dry_run,
            json,
        } => {
            let modes = [
                select.is_some(),
                delete.is_some(),
                export.is_some(),
                import.is_some(),
            ]
            .iter()
            .filter(|x| **x)
            .count();
            if modes > 1 {
                return Err(DraftError::invalid_config(
                    "draft pack accepts only one of --select, --delete, --export, --import",
                ));
            }
            if let Some(reference) = export {
                render_json_or_text(
                    app.pack_export(cwd, &reference, output.as_deref().map(Path::new))?,
                    json,
                    "Pack exported",
                )
            } else if let Some(artifact) = import {
                render_json_or_text(
                    app.pack_import(cwd, Path::new(&artifact), name.as_deref(), dry_run)?,
                    json,
                    if dry_run {
                        "Import dry run"
                    } else {
                        "Pack imported to quarantine"
                    },
                )
            } else if let Some(reference) = select {
                render_json_or_text(
                    app.pack_select_ref(cwd, &reference)?,
                    json,
                    "ChangePack selected",
                )
            } else if let Some(reference) = delete {
                let report = app.pack_show(cwd, &reference)?;
                if !confirm_pack_delete(&report.pack)? {
                    return Err(DraftError::invalid_config("ChangePack deletion aborted"));
                }
                render_json_or_text(
                    app.pack_delete_ref(cwd, &reference)?,
                    json,
                    "ChangePack deleted",
                )
            } else {
                render_json_or_text(app.pack_show_selected(cwd)?, json, "ChangePack")
            }
        }
        PackCommand::List { json } => render_json_or_text(app.pack_list(cwd)?, json, "ChangePacks"),
        PackCommand::Candidate { action } => match action {
            CandidateAction::List { json } => {
                render_json_or_text(app.candidate_list(cwd)?, json, "Candidates")
            }
            CandidateAction::Show {
                candidate_name,
                json,
            } => render_json_or_text(app.candidate_show(cwd, &candidate_name)?, json, "Candidate"),
            CandidateAction::Add(args) => render_json_or_text(
                app.candidate_add(
                    cwd,
                    &args.candidate_name,
                    args.kind.as_deref(),
                    args.template,
                )?,
                args.json,
                "Candidate added",
            ),
            CandidateAction::Update(args) => render_json_or_text(
                app.candidate_update(
                    cwd,
                    &args.candidate_name,
                    args.kind.as_deref(),
                    args.template,
                )?,
                args.json,
                "Candidate updated",
            ),
            CandidateAction::Remove {
                candidate_name,
                json,
            } => render_json_or_text(
                app.candidate_remove(cwd, &candidate_name)?,
                json,
                "Candidate removed",
            ),
            CandidateAction::Packs {
                pack,
                candidate,
                json,
            } => render_json_or_text(
                app.candidate_packs(cwd, pack.as_deref(), candidate.as_deref())?,
                json,
                "Candidate packs",
            ),
        },
    }
}

fn run_review(app: &App, cwd: &Path, command: ReviewCommand) -> Result<(), DraftError> {
    match command {
        ReviewCommand::Verify {
            target,
            pack,
            explain,
            full,
            fuzz,
            json,
        } => {
            // v0.3.3 evidence verification triggers on a positional target or an
            // evidence flag; legacy `verify -p <pack>` stays on the legacy path.
            if target.is_some() || explain || full || fuzz {
                let reference = target.or(pack);
                let refstr = reference.as_deref().unwrap_or("selected");
                // Legacy changepacks also need the legacy verification receipt
                // so the legacy submit gate passes (same as `verify -p`).
                // Imported packs are canonical-only and skip it.
                if app.is_legacy_pack_ref(cwd, refstr) {
                    app.verify_selected(cwd, Some(refstr))?;
                }
                let report = app.verify_pack_v2(cwd, refstr, full, fuzz)?;
                render_verify(report, explain, json)
            } else {
                let reference = app.resolve_pack_arg(cwd, pack.as_deref())?;
                render_json_or_text(
                    {
                        let report = app.verify_selected(cwd, Some(&reference))?;
                        app.verify_pack_v2(cwd, &reference, false, false)?;
                        report
                    },
                    json,
                    "Verification complete",
                )
            }
        }
        ReviewCommand::Risk {
            pack,
            explain,
            include_evidence,
            json,
        } => render_json_or_text(
            app.risk_selected_with_options(cwd, pack.as_deref(), explain, include_evidence)?,
            json,
            "Risk assessed",
        ),
        ReviewCommand::Review {
            pack,
            tui,
            comment,
            json,
        } => {
            if tui {
                return draft_tui::run_console(cwd)
                    .map_err(|e| DraftError::new(DraftErrorKind::Internal, e));
            }
            render_json_or_text(
                app.review_selected(cwd, pack.as_deref(), comment)?,
                json,
                "Review recorded",
            )
        }
        ReviewCommand::Approve { pack, reason, json } => render_json_or_text(
            app.decide_pack(
                cwd,
                app.resolve_pack_arg(cwd, pack.as_deref())?.as_str(),
                true,
                reason,
            )?,
            json,
            "ChangePack approved",
        ),
        ReviewCommand::Reject { pack, reason, json } => render_json_or_text(
            app.decide_pack(
                cwd,
                app.resolve_pack_arg(cwd, pack.as_deref())?.as_str(),
                false,
                reason,
            )?,
            json,
            "ChangePack rejected",
        ),
    }
}

fn run_integration(app: &App, cwd: &Path, command: IntegrationCommand) -> Result<(), DraftError> {
    match command {
        IntegrationCommand::Compare {
            left,
            right,
            tui,
            json,
        } => {
            if tui {
                return draft_tui::run_console(cwd)
                    .map_err(|e| DraftError::new(DraftErrorKind::Internal, e));
            }
            render_json_or_text(app.compare(cwd, &left, &right)?, json, "Compare complete")
        }
        IntegrationCommand::Compose {
            left,
            right,
            output: out,
            tui,
            json,
        } => render_json_or_text(
            {
                if tui {
                    return draft_tui::run_console(cwd)
                        .map_err(|e| DraftError::new(DraftErrorKind::Internal, e));
                }
                app.compose(cwd, &left, &right, &out)?
            },
            json,
            "Compose complete",
        ),
        IntegrationCommand::Disperse {
            pack,
            output,
            tui,
            json,
        } => {
            if tui {
                return draft_tui::run_console(cwd)
                    .map_err(|e| DraftError::new(DraftErrorKind::Internal, e));
            }
            render_json_or_text(
                app.disperse(cwd, &pack, &output[0], &output[1])?,
                json,
                "Disperse complete",
            )
        }
        IntegrationCommand::Submit {
            pack,
            vars,
            dry_run,
            json,
        } => {
            if dry_run {
                render_dry_run(app.submit_dry_run(cwd, pack.as_deref())?, json)
            } else {
                let vars = draft_core::parse_hook_vars(vars)?;
                render_json_or_text(
                    app.submit_selected(cwd, pack.as_deref(), vars)?,
                    json,
                    "ChangePack submitted",
                )
            }
        }
        IntegrationCommand::Rollback {
            reference,
            dry_run,
            json,
        } => {
            if dry_run {
                render_dry_run(app.rollback_dry_run(cwd, &reference)?, json)
            } else {
                render_json_or_text(
                    app.rollback(cwd, &reference, true)?,
                    json,
                    "Rollback complete",
                )
            }
        }
        IntegrationCommand::Receipt { action } => match action {
            ReceiptAction::List { json } => {
                render_json_or_text(app.receipts(cwd)?, json, "Receipts")
            }
            ReceiptAction::Show { receipt_id, json } => {
                render_receipt_show(app.receipt_show(cwd, &receipt_id)?, json)
            }
            ReceiptAction::Verify {
                receipt_id,
                all,
                json,
            } => {
                if all {
                    let v = app.receipt_verify_all(cwd)?;
                    render_ledger_verification(v, json)
                } else if let Some(id) = receipt_id {
                    let v = app.receipt_verify(cwd, &id)?;
                    render_receipt_verification(v, json)
                } else {
                    Err(DraftError::invalid_config(
                        "provide a receipt id (rcp_...) or --all",
                    ))
                }
            }
        },
    }
}

fn run_maintenance(app: &App, cwd: &Path, command: MaintenanceCommand) -> Result<(), DraftError> {
    match command {
        MaintenanceCommand::Close { force } => render_close(app.close(cwd, force)?),
        MaintenanceCommand::Gc => render_gc(app.gc(cwd)?),
        MaintenanceCommand::Storage { action } => match action {
            StorageAction::Stats { json } => {
                render_json_or_text(app.storage_stats(cwd)?, json, "Storage stats")
            }
            StorageAction::Gc { json } => {
                render_json_or_text(app.storage_gc(cwd)?, json, "Storage GC complete")
            }
            StorageAction::Compact { json } => {
                render_json_or_text(app.storage_compact(cwd)?, json, "Storage compact complete")
            }
            StorageAction::Prune { json } => {
                render_json_or_text(app.storage_prune(cwd)?, json, "Storage prune complete")
            }
            StorageAction::Doctor { json } => {
                render_json_or_text(app.storage_doctor(cwd)?, json, "Storage doctor complete")
            }
        },
    }
}

fn ensure_project_scope(command: &Command, cwd: &Path) -> Result<(), DraftError> {
    if !requires_project_scope(command) || find_workspace_root(cwd).is_some() {
        return Ok(());
    }
    Err(DraftError::new(
        DraftErrorKind::ProjectScopeRequired,
        "this command must be run inside a Draft workspace",
    )
    .with_context(format!("current directory: {}", cwd.display()))
    .with_suggestion("run `draft init` here or change into an existing Draft workspace"))
}

fn requires_project_scope(command: &Command) -> bool {
    match command {
        Command::Workspace(command) => requires_workspace_scope(command),
        Command::Maintenance(MaintenanceCommand::Storage {
            action: StorageAction::Doctor { .. },
        }) => false,
        _ => true,
    }
}

fn requires_workspace_scope(command: &WorkspaceCommand) -> bool {
    match command {
        WorkspaceCommand::Init { .. } => false,
        WorkspaceCommand::Doctor {
            action: Some(DoctorAction::Sync { .. }),
            ..
        }
        | WorkspaceCommand::Doctor { global: true, .. } => false,
        WorkspaceCommand::Doctor { .. } => true,
        WorkspaceCommand::Identity { .. } => false,
        WorkspaceCommand::Extension { .. } => false,
        WorkspaceCommand::Config {
            action:
                Some(ConfigAction::Get { global: true, .. })
                | Some(ConfigAction::Set { global: true, .. })
                | Some(ConfigAction::Unset { global: true, .. })
                | None,
            ..
        } => false,
        WorkspaceCommand::Config { .. } => true,
        WorkspaceCommand::Status { .. } => false,
        _ => true,
    }
}

fn find_workspace_root(cwd: &Path) -> Option<PathBuf> {
    let mut cur = cwd.to_path_buf();
    loop {
        if cur.join(".draft").join("workspace.json").exists() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

fn render_init(report: draft_core::InitReport, json: bool) -> Result<(), DraftError> {
    if json {
        output::print_json(&report);
        return Ok(());
    }
    if report.created {
        output::success("Initialized Draft workspace.");
    } else {
        output::warn("Draft workspace already initialized here.");
    }
    output::field("Workspace", &report.workspace_id);
    output::field("Root", &report.root);
    output::field(".draft", &report.draft_dir);
    output::field("Stable head", &report.stable_head_id);
    output::field("Stable receipt", &report.stable_head_receipt_id);
    output::field("Workspace hash", &report.workspace_hash);
    if !report.next_actions.is_empty() {
        output::section("Next actions");
        for action in &report.next_actions {
            output::bullet(action);
        }
    }
    if !report.candidate_guidance.is_empty() {
        output::section("Candidates");
        output::line(&report.candidate_guidance);
    }
    Ok(())
}

fn render_init_global(report: draft_core::InitGlobalReport, json: bool) -> Result<(), DraftError> {
    if json {
        output::print_json(&report);
        return Ok(());
    }
    if report.created {
        output::success("Initialized global Draft store.");
    } else {
        output::warn("Global Draft store already initialized.");
    }
    output::field("Root", &report.root);
    output::field("Hidden", &report.hidden.to_string());
    output::field("Actor", &report.actor_id);
    output::field("Public key", &report.public_key_id);
    Ok(())
}

fn render_doctor(report: draft_core::DoctorReport, json: bool) -> Result<(), DraftError> {
    if json {
        output::print_json(&report);
    } else {
        print_doctor_scope(&report.global);
        if let Some(project) = &report.project {
            print_doctor_scope(project);
        }
    }
    if report.healthy() {
        Ok(())
    } else {
        Err(DraftError::new(
            DraftErrorKind::Storage,
            "draft doctor found problems",
        ))
    }
}

fn print_doctor_scope(scope: &draft_core::DoctorScope) {
    output::header(&format!("{} store", scope.label));
    output::field("Root", &scope.root);
    output::field("Exists", &scope.exists.to_string());
    output::field("Hidden", &scope.hidden.to_string());
    for c in &scope.checks {
        let mark = if c.ok { "ok " } else { "FAIL" };
        println!("  [{mark}] {:<18} {}", c.name, c.detail);
    }
}

fn render_verify(
    report: draft_core::VerifyReport,
    explain: bool,
    json: bool,
) -> Result<(), DraftError> {
    if json {
        output::print_json(&report);
        return Ok(());
    }
    output::header(&format!("Verified {}", report.pack_id));
    output::field(
        "Risk",
        &format!("{} ({}/100)", report.risk_level, report.risk_score),
    );
    output::field("Symbols touched", &report.symbols_touched.to_string());
    output::field("Public API changed", &report.public_api_changed.to_string());
    output::field("Selected tests", &report.selected_tests.len().to_string());
    output::field(
        "Selected fuzz targets",
        &report.selected_fuzz_targets.len().to_string(),
    );
    output::field("Result hash", &report.result_hash);
    if explain {
        println!("\nSelection: {}", report.selection_reason);
        println!("Coverage:  {}", report.coverage_basis);
        for t in &report.selected_tests {
            println!("  test {} — {} ({})", t.name, t.command, t.reason);
        }
        for f in &report.selected_fuzz_targets {
            println!("  fuzz {} — {} ({})", f.name, f.command, f.reason);
        }
        if !report.explanations.is_empty() {
            println!("\nRisk explanations:");
            for e in &report.explanations {
                println!("  - {e}");
            }
        }
        if !report.required_actions.is_empty() {
            println!("Required actions:");
            for a in &report.required_actions {
                println!("  - {a}");
            }
        }
    }
    Ok(())
}

fn render_dry_run(report: draft_core::DryRunReport, json: bool) -> Result<(), DraftError> {
    if json {
        output::print_json(&report);
        return Ok(());
    }
    output::header(&format!("Dry run: {} {}", report.action, report.target));
    output::field("Would proceed", &report.would_proceed.to_string());
    output::field("Resulting state", &report.resulting_state);
    for c in &report.checks {
        let mark = if c.ok { "ok " } else { "FAIL" };
        println!("  [{mark}] {:<18} {}", c.name, c.detail);
    }
    if !report.affected_files.is_empty() {
        output::field("Affected files", &report.affected_files.len().to_string());
        for f in &report.affected_files {
            println!("    {f}");
        }
    }
    Ok(())
}

fn render_receipt_show(value: serde_json::Value, json: bool) -> Result<(), DraftError> {
    if json {
        output::print_json(&value);
        return Ok(());
    }
    let receipt_id = json_string(&value, &["receipt_id", "id"]).unwrap_or("unknown");
    output::header(&format!("Receipt {receipt_id}"));
    output::section("Proof");
    if let Some(event_type) = json_string(&value, &["event_type", "kind"]) {
        output::bullet(&format!("Event: {event_type}"));
    }
    if let Some(subject) = json_string(&value, &["subject_id", "changepack_id", "pack_id"]) {
        output::bullet(&format!("Subject: {subject}"));
    }
    if let Some(actor) = json_string(&value, &["actor_id", "actor"]) {
        output::bullet(&format!("Actor: {actor}"));
    }
    if let Some(workspace_hash) = json_string(&value, &["workspace_hash"]) {
        output::bullet(&format!("Workspace hash: {workspace_hash}"));
    }
    if let Some(public_key) = json_string(&value, &["public_key_id"]) {
        output::bullet(&format!("Public key: {public_key}"));
    }
    if json_string(&value, &["signature"]).is_some() {
        output::bullet("Signature: present");
    }
    output::section("Receipt IDs");
    output::bullet(&format!("Receipt: {receipt_id}"));
    if let Some(event_hash) = json_string(&value, &["event_hash"]) {
        output::bullet(&format!("Event hash: {event_hash}"));
    }
    if let Some(previous) = json_string(&value, &["previous_event_hash"]) {
        output::bullet(&format!("Previous event hash: {previous}"));
    }
    Ok(())
}

fn json_string<'a>(value: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| value.get(*key)?.as_str())
}

fn render_receipt_verification(
    v: draft_core::receipt::ReceiptVerification,
    json: bool,
) -> Result<(), DraftError> {
    if json {
        output::print_json(&v);
    } else {
        output::header(&format!("Receipt {}", v.receipt_id));
        for c in &v.checks {
            let mark = if c.ok { "ok " } else { "FAIL" };
            println!("  [{mark}] {:<20} {}", c.name, c.detail);
        }
    }
    if v.ok {
        Ok(())
    } else {
        Err(DraftError::new(
            DraftErrorKind::Storage,
            format!("receipt {} failed verification", v.receipt_id),
        ))
    }
}

fn render_ledger_verification(
    v: draft_core::ledger::LedgerVerification,
    json: bool,
) -> Result<(), DraftError> {
    if json {
        output::print_json(&v);
    } else {
        output::header("Trust ledger verification");
        println!(
            "  event chain:   {} ({} events)",
            ok_word(v.event_chain_ok),
            v.event_count
        );
        println!(
            "  transparency:  {} ({} entries)",
            ok_word(v.transparency_ok),
            v.transparency_count
        );
        let bad = v.receipts.iter().filter(|r| !r.ok).count();
        println!(
            "  receipts:      {} ({} ok / {} failed)",
            ok_word(bad == 0),
            v.receipts.len() - bad,
            bad
        );
    }
    if v.all_ok {
        Ok(())
    } else {
        Err(DraftError::new(
            DraftErrorKind::Storage,
            "trust ledger failed verification",
        ))
    }
}

fn render_close(report: draft_core::CloseReport) -> Result<(), DraftError> {
    output::success("Draft closed");
    output::field(".draft removed", &report.draft_dir);
    output::field("Forced", &report.forced.to_string());
    output::field("Pending packs", &report.pending_packs.to_string());
    Ok(())
}

fn render_gc(report: draft_core::gc::GcReport) -> Result<(), DraftError> {
    output::success("Draft GC complete");
    output::field("Removed entries", &report.removed_entries.to_string());
    output::field("Stable head valid", &report.stable_head_valid.to_string());
    output::field(
        "Active packs preserved",
        &report.active_packs_preserved.to_string(),
    );
    Ok(())
}

fn ok_word(ok: bool) -> &'static str {
    if ok {
        "OK"
    } else {
        "FAIL"
    }
}

fn render_identity(
    status: draft_core::identity::IdentityStatus,
    json: bool,
) -> Result<(), DraftError> {
    if json {
        output::print_json(&status);
        return Ok(());
    }
    match &status.actor {
        Some(a) => {
            output::field("Actor", &a.actor_id);
            output::field("Name", &a.display_name);
            output::field("Public key", &a.public_key_id);
        }
        None => output::warn("No actor; run `draft init --global`."),
    }
    output::field("Signing key", &status.signing_key_available.to_string());
    output::field("Candidates", &status.candidate_count.to_string());
    Ok(())
}

fn render_config(report: draft_core::ConfigReport, json: bool) -> Result<(), DraftError> {
    if json {
        output::print_json(&report);
        return Ok(());
    }
    for (k, v) in report.entries {
        output::field(&k, &v);
    }
    Ok(())
}

fn render_ignore(report: draft_core::IgnoreReport, json: bool) -> Result<(), DraftError> {
    if json {
        output::print_json(&report);
        return Ok(());
    }
    for p in report.patterns {
        println!("{p}");
    }
    Ok(())
}

fn render_status(report: draft_core::WorkspaceStatus, json: bool) -> Result<(), DraftError> {
    if json {
        output::print_json(&report);
        return Ok(());
    }
    output::header("Workspace Status");
    output::field("Workspace", &report.workspace_id.to_string());
    output::field("Changes", &report.changes.len().to_string());
    for c in report.changes {
        println!("  {:<12} {}", format!("{:?}", c.change_kind), c.path);
    }
    Ok(())
}

fn render_status_report(report: draft_core::StatusReport, json: bool) -> Result<(), DraftError> {
    if json {
        output::print_json(&report);
        return Ok(());
    }
    output::header("Workspace Status");
    output::field("Workspace", &report.workspace.workspace_id.to_string());
    output::field("Changes", &report.workspace.changes.len().to_string());
    if let Some(component) = &report.component {
        output::field("Component", component);
    }
    if let Some(pack) = &report.pack {
        output::field("Pack", pack);
    }
    for (name, value) in report.sections {
        output::section(&name);
        output::print_human(&value);
    }
    Ok(())
}

fn render_events(
    events: Vec<draft_core::EventEnvelope>,
    json: bool,
    raw: bool,
) -> Result<(), DraftError> {
    if json {
        output::print_json(&events);
        return Ok(());
    }
    if raw {
        for e in events {
            println!("{}", serde_json::to_string(&e).map_err(DraftError::from)?);
        }
        return Ok(());
    }
    for e in events {
        println!(
            "{} {} {}",
            e.time.to_rfc3339(),
            e.event_type,
            e.subject_id.unwrap_or_default()
        );
    }
    Ok(())
}

fn run_task_wizard(
    app: &App,
    cwd: &Path,
    json: bool,
) -> Result<draft_core::task::TaskDefinition, DraftError> {
    let name = prompt_line("Task name", json)?;
    let template = prompt_line("Task type/template", json)?;
    let goal = prompt_line("Goal", json)?;
    let should_not_change = prompt_line("What should not change?", json)?;
    let allowed = prompt_line("Allowed zones (comma-separated)", json)?;
    let forbidden = prompt_line("Forbidden zones (comma-separated)", json)?;
    let success = prompt_line("Success checks (comma-separated)", json)?;
    let risk = prompt_line("Risk [low|medium|high|critical]", json)?;
    let plan_first = prompt_line("Plan first? [y/N]", json)?;
    let candidate_preset = prompt_line("Candidate preset", json)?;
    let mode = if matches!(plan_first.trim(), "y" | "Y" | "yes" | "YES") {
        Some("plan-first".to_string())
    } else {
        None
    };
    let mut forbidden_zones = split_csv(&forbidden);
    forbidden_zones.extend(split_csv(&should_not_change));
    if !json {
        output::section("Preview");
        output::field("Name", name.trim());
        output::field("Template", template.trim());
        output::field("Goal", goal.trim());
        output::field("Allowed", &split_csv(&allowed).join(", "));
        output::field("Forbidden", &forbidden_zones.join(", "));
        output::field("Success", &split_csv(&success).join(", "));
        output::field("Risk", risk.trim());
        output::field("Mode", mode.as_deref().unwrap_or("normal"));
        output::field("Preset", candidate_preset.trim());
    }
    let confirm = prompt_line("Create task? [y/N]", json)?;
    if !matches!(confirm.trim(), "y" | "Y" | "yes" | "YES") {
        return Err(DraftError::invalid_config("task wizard cancelled"));
    }
    let template = (!template.trim().is_empty()).then(|| template.trim().to_string());
    let risk = (!risk.trim().is_empty()).then(|| risk.trim().to_string());
    let candidate_preset =
        (!candidate_preset.trim().is_empty()).then(|| candidate_preset.trim().to_string());
    app.task_define(
        cwd,
        name.trim(),
        goal.trim(),
        template,
        split_csv(&allowed),
        forbidden_zones,
        split_csv(&success),
        risk.as_deref(),
        mode.as_deref(),
        candidate_preset,
    )
}

fn split_csv(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn prompt_line(label: &str, quiet: bool) -> Result<String, DraftError> {
    if !quiet {
        print!("{label}: ");
        io::stdout().flush().map_err(DraftError::from)?;
    }
    let mut buf = String::new();
    io::stdin().read_line(&mut buf).map_err(DraftError::from)?;
    Ok(buf.trim_end().to_string())
}

fn render_json_or_text<T: serde::Serialize>(
    value: T,
    json: bool,
    label: &str,
) -> Result<(), DraftError> {
    if json {
        output::print_json(&value);
    } else {
        // Human-readable by default (SRS-FR-130/131): never JSON without a flag.
        output::success(label);
        output::print_human(&value);
    }
    Ok(())
}

fn confirm_pack_delete(pack: &draft_core::Changepack) -> Result<bool, DraftError> {
    let name = pack.name.as_deref().unwrap_or("<unnamed>");
    print!("Delete ChangePack {name} ({})? [y/N]: ", pack.id);
    io::stdout().flush().map_err(DraftError::from)?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(DraftError::from)?;
    Ok(matches!(input.trim(), "y" | "Y"))
}

#[allow(dead_code)]
fn _assert_path(_: &Path) {}
