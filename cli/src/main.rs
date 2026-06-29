mod output;
mod service;

use std::path::Path;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use draft_core::error::{DraftError, DraftErrorKind};
use draft_core::{App, DecisionKind};

#[derive(Parser)]
#[command(name = "draft", version = draft_core::DRAFT_VERSION, about = "Draft v0.3.0 - Verified Changepacks + Review Cockpit")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a Draft workspace.
    Init {
        #[arg(long)]
        json: bool,
    },
    /// Manage workspace config.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage .draft/.ignore.
    Ignore {
        #[command(subcommand)]
        action: IgnoreAction,
    },
    /// Show Draft-native workspace status.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Show append-only events.
    Events {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        verify_chain: bool,
    },
    /// Alias for events.
    Log {
        #[arg(long)]
        json: bool,
    },
    /// Manage rebuildable indexes.
    Index {
        #[command(subcommand)]
        action: IndexAction,
    },
    /// Manage tasks.
    Task {
        #[command(subcommand)]
        action: TaskAction,
    },
    /// Create a Draft-native checkpoint.
    Checkpoint {
        message: String,
        #[arg(long)]
        json: bool,
    },
    /// Manage changepacks.
    Pack {
        #[command(subcommand)]
        action: PackAction,
    },
    /// Run an opaque shell/agent command and capture evidence.
    Spawn(SpawnArgs),
    /// Inspect runs.
    Runs {
        #[command(subcommand)]
        action: Option<RunsAction>,
        #[arg(long)]
        json: bool,
    },
    /// Run configured verification checks.
    Verify {
        pack: String,
        #[arg(long)]
        json: bool,
    },
    /// Assess risk.
    Risk {
        pack: String,
        #[arg(long)]
        json: bool,
    },
    /// Review a changepack or launch the TUI.
    Review {
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
        pack: String,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Reject a changepack.
    Reject {
        pack: String,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Compare changepacks.
    Compare {
        left: Option<String>,
        right: Option<String>,
        #[arg(long)]
        tui: Option<String>,
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
        json: bool,
    },
    /// Save an approved changepack into .draft/ and optionally target.local.
    Save {
        pack: String,
        #[arg(long)]
        json: bool,
    },
    /// Roll back to a checkpoint, pack base snapshot, or receipt.
    Rollback {
        target: String,
        #[arg(long)]
        plan: bool,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    /// Inspect receipts.
    Receipt {
        #[command(subcommand)]
        action: ReceiptAction,
    },
    /// Manage the optional local service.
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    Set {
        key: String,
        value: String,
        #[arg(long)]
        json: bool,
    },
    Get {
        key: String,
        #[arg(long)]
        json: bool,
    },
    Unset {
        key: String,
        #[arg(long)]
        json: bool,
    },
    List {
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
    Create {
        title: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    Show {
        task_id: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum IndexAction {
    Rebuild {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum PackAction {
    Create {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        from_working_tree: bool,
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    Show {
        pack: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct SpawnArgs {
    #[arg(long)]
    task: String,
    #[arg(long)]
    name: String,
    #[arg(last = true, required = true)]
    command: Vec<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum RunsAction {
    Show { run_id: String },
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
}

#[derive(Subcommand)]
pub enum ServiceAction {
    Start,
    Stop,
    Status {
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
                DraftErrorKind::VerificationFailed
                | DraftErrorKind::RiskPolicyBlocked
                | DraftErrorKind::ReviewRequired
                | DraftErrorKind::ConflictDetected
                | DraftErrorKind::SaveFailed => ExitCode::from(2),
                _ => ExitCode::FAILURE,
            }
        }
    }
}

fn run(cli: Cli) -> Result<(), DraftError> {
    let cwd = std::env::current_dir().map_err(DraftError::from)?;
    let app = App::new();
    match cli.command {
        Command::Init { json } => render_init(app.init(&cwd)?, json),
        Command::Config { action } => match action {
            ConfigAction::Set { key, value, json } => {
                render_config(app.config_set(&cwd, &key, &value)?, json)
            }
            ConfigAction::Get { key, json } => render_config(app.config_get(&cwd, &key)?, json),
            ConfigAction::Unset { key, json } => render_config(app.config_unset(&cwd, &key)?, json),
            ConfigAction::List { json } => render_config(app.config_list(&cwd)?, json),
        },
        Command::Ignore { action } => match action {
            IgnoreAction::Add { pattern, json } => {
                render_ignore(app.ignore_add(&cwd, &pattern)?, json)
            }
            IgnoreAction::Remove { pattern, json } => {
                render_ignore(app.ignore_remove(&cwd, &pattern)?, json)
            }
            IgnoreAction::List { json } => render_ignore(app.ignore_list(&cwd)?, json),
        },
        Command::Status { json } => render_status(app.status(&cwd)?, json),
        Command::Events { json, verify_chain } => {
            if verify_chain {
                render_json_or_text(app.verify_events(&cwd)?, json, "Event chain verified")
            } else {
                render_events(app.events(&cwd)?, json)
            }
        }
        Command::Log { json } => render_events(app.events(&cwd)?, json),
        Command::Index { action } => match action {
            IndexAction::Rebuild { json } => {
                render_json_or_text(app.index_rebuild(&cwd)?, json, "Index rebuilt")
            }
        },
        Command::Task { action } => match action {
            TaskAction::Create {
                title,
                description,
                json,
            } => render_json_or_text(
                app.task_create(&cwd, &title, description)?,
                json,
                "Task created",
            ),
            TaskAction::List { json } => render_json_or_text(app.task_list(&cwd)?, json, "Tasks"),
            TaskAction::Show { task_id, json } => {
                render_json_or_text(app.task_show(&cwd, &task_id)?, json, "Task")
            }
        },
        Command::Checkpoint { message, json } => {
            render_json_or_text(app.checkpoint(&cwd, &message)?, json, "Checkpoint created")
        }
        Command::Pack { action } => match action {
            PackAction::Create {
                name,
                from_working_tree,
                task,
                json,
            } => render_json_or_text(
                app.pack_create(&cwd, name, task, from_working_tree)?,
                json,
                "Changepack created",
            ),
            PackAction::List { json } => {
                render_json_or_text(app.pack_list(&cwd)?, json, "Changepacks")
            }
            PackAction::Show { pack, json } => {
                render_json_or_text(app.pack_show(&cwd, &pack)?, json, "Changepack")
            }
        },
        Command::Spawn(args) => render_json_or_text(
            app.spawn_run(&cwd, &args.task, &args.name, args.command)?,
            args.json,
            "Run completed",
        ),
        Command::Runs { action, json } => match action {
            Some(RunsAction::Show { run_id }) => {
                render_json_or_text(app.run_show(&cwd, &run_id)?, json, "Run")
            }
            None => render_json_or_text(app.runs(&cwd)?, json, "Runs"),
        },
        Command::Verify { pack, json } => {
            render_json_or_text(app.verify(&cwd, &pack)?, json, "Verification complete")
        }
        Command::Risk { pack, json } => {
            render_json_or_text(app.risk(&cwd, &pack)?, json, "Risk assessed")
        }
        Command::Review {
            pack,
            tui,
            comment,
            json,
        } => {
            if tui {
                return draft_tui::run_review_cockpit(&cwd)
                    .map_err(|e| DraftError::new(DraftErrorKind::Internal, e));
            }
            let pack = pack.ok_or_else(|| {
                DraftError::invalid_config("review requires <pack> unless --tui is used")
            })?;
            render_json_or_text(app.review(&cwd, &pack, comment)?, json, "Review recorded")
        }
        Command::Approve { pack, reason, json } => render_json_or_text(
            app.decide(&cwd, &pack, DecisionKind::Approve, reason)?,
            json,
            "Changepack approved",
        ),
        Command::Reject { pack, reason, json } => render_json_or_text(
            app.decide(&cwd, &pack, DecisionKind::Reject, reason)?,
            json,
            "Changepack rejected",
        ),
        Command::Compare {
            left,
            right,
            tui,
            json,
        } => {
            if tui.is_some() {
                return draft_tui::run_review_cockpit(&cwd)
                    .map_err(|e| DraftError::new(DraftErrorKind::Internal, e));
            }
            render_json_or_text(
                app.compare(&cwd, &left.unwrap_or_default(), &right.unwrap_or_default())?,
                json,
                "Compare complete",
            )
        }
        Command::Compose {
            left,
            right,
            output: out,
            json,
        } => render_json_or_text(
            app.compose(&cwd, &left, &right, &out)?,
            json,
            "Compose complete",
        ),
        Command::Save { pack, json } => {
            render_json_or_text(app.save(&cwd, &pack)?, json, "Changepack saved")
        }
        Command::Rollback {
            target,
            plan,
            yes,
            json,
        } => {
            if plan {
                render_json_or_text(app.rollback_plan(&cwd, &target)?, json, "Rollback plan")
            } else {
                render_json_or_text(app.rollback(&cwd, &target, yes)?, json, "Rollback complete")
            }
        }
        Command::Receipt { action } => match action {
            ReceiptAction::List { json } => {
                render_json_or_text(app.receipts(&cwd)?, json, "Receipts")
            }
            ReceiptAction::Show { receipt_id, json } => {
                render_json_or_text(app.receipt_show(&cwd, &receipt_id)?, json, "Receipt")
            }
        },
        Command::Service { action } => service::handle(action, &cwd),
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

fn render_events(events: Vec<draft_core::EventEnvelope>, json: bool) -> Result<(), DraftError> {
    if json {
        output::print_json(&events);
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

fn render_json_or_text<T: serde::Serialize>(
    value: T,
    json: bool,
    label: &str,
) -> Result<(), DraftError> {
    if json {
        output::print_json(&value);
    } else {
        output::success(label);
        output::print_json(&value);
    }
    Ok(())
}

#[allow(dead_code)]
fn _assert_path(_: &Path) {}
