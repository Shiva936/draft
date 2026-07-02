mod output;

use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use draft_core::error::{DraftError, DraftErrorKind};
use draft_core::{App, DecisionKind};

#[derive(Parser)]
#[command(name = "draft", version = draft_core::DRAFT_VERSION, about = "Draft v0.3.1 - Compatibility + Agent Review Scaling")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a Draft workspace.
    Init {
        #[arg(short = 'b')]
        base: Option<String>,
        #[arg(long)]
        json: bool,
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
        top: bool,
        #[arg(long)]
        bottom: bool,
        #[arg(long)]
        page: Option<usize>,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(short = 'f', long)]
        filter: Option<String>,
        #[arg(long)]
        raw: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        verify_chain: bool,
    },
    /// Manage tasks.
    Task {
        #[command(subcommand)]
        action: Option<TaskAction>,
    },
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
    /// Show, switch, or delete the selected ChangePack.
    Pack {
        #[arg(short = 's')]
        select: Option<String>,
        #[arg(short = 'd')]
        delete: Option<String>,
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
    /// Run configured verification checks.
    Verify {
        #[arg(short = 'p')]
        pack: Option<String>,
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
    /// Save an approved changepack into .draft/ and optionally run hooks.save.
    Save {
        #[arg(short = 'p')]
        pack: Option<String>,
        #[arg(long = "var", num_args = 1.., allow_hyphen_values = true, value_name = "key=value")]
        vars: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Roll back to a checkpoint, pack base snapshot, or receipt.
    Rollback {
        reference: String,
        #[arg(long)]
        json: bool,
    },
    /// Inspect receipts.
    Receipt {
        #[command(subcommand)]
        action: ReceiptAction,
    },
    /// Manage storage.
    Storage {
        #[command(subcommand)]
        action: StorageAction,
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
    Unset {
        key: String,
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
    Spawn {
        name: String,
        #[arg(short = 'p')]
        pack: Option<String>,
        #[arg(short = 'c')]
        candidates: Vec<String>,
        #[arg(long)]
        cron: Option<String>,
        #[arg(last = true, required = true)]
        instruction: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    #[command(external_subcommand)]
    External(Vec<String>),
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
        Command::Init { base, json } => render_init(
            app.init_with_base(&cwd, base.as_deref().unwrap_or("base"))?,
            json,
        ),
        Command::Config { key, action } => match action {
            Some(ConfigAction::Set { key, value, json }) => {
                render_config(app.config_set(&cwd, &key, &value)?, json)
            }
            Some(ConfigAction::Unset { key, json }) => {
                render_config(app.config_unset(&cwd, &key)?, json)
            }
            None => {
                if let Some(key) = key {
                    render_config(app.config_get(&cwd, &key)?, false)
                } else {
                    render_config(app.config_list(&cwd)?, false)
                }
            }
        },
        Command::Hook { key, action } => match action {
            Some(HookAction::Set { key, value, json }) => {
                render_config(app.hook_set(&cwd, &key, &value)?, json)
            }
            Some(HookAction::Unset { key, json }) => {
                render_config(app.hook_unset(&cwd, &key)?, json)
            }
            Some(HookAction::Run { hook_name, json }) => {
                render_json_or_text(app.hook_run(&cwd, &hook_name)?, json, "Hook complete")
            }
            None => {
                if let Some(key) = key {
                    render_config(app.hook_get(&cwd, &key)?, false)
                } else {
                    render_config(app.hook_list(&cwd)?, false)
                }
            }
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
        Command::Status {
            pack,
            component,
            full,
            json,
        } => render_status(
            app.status_v031(&cwd, pack.as_deref(), component.as_deref(), full)?,
            json,
        ),
        Command::Event {
            top,
            bottom,
            page,
            limit,
            filter,
            raw,
            json,
            verify_chain,
        } => {
            if verify_chain {
                render_json_or_text(app.verify_events(&cwd)?, json, "Event chain verified")
            } else {
                render_events(
                    app.events_page(&cwd, top, bottom, page, limit, filter.as_deref())?,
                    json,
                    raw,
                )
            }
        }
        Command::Task { action } => match action {
            Some(TaskAction::Spawn {
                name,
                pack,
                candidates,
                cron,
                instruction,
                json,
            }) => render_json_or_text(
                app.task_spawn(&cwd, &name, pack.as_deref(), candidates, cron, instruction)?,
                json,
                "Task spawned",
            ),
            Some(TaskAction::List { json }) => {
                render_json_or_text(app.task_list(&cwd)?, json, "Tasks")
            }
            Some(TaskAction::External(args)) => {
                let task_id = args
                    .first()
                    .ok_or_else(|| DraftError::invalid_config("missing task id"))?;
                render_json_or_text(app.task_show(&cwd, task_id)?, false, "Task")
            }
            None => render_json_or_text(app.task_current(&cwd)?, false, "Task"),
        },
        Command::Checkpoint { message, json } => {
            render_json_or_text(app.checkpoint(&cwd, &message)?, json, "Checkpoint created")
        }
        Command::Create {
            name,
            base_pack,
            json,
        } => render_json_or_text(
            app.pack_create_from_base(&cwd, name, base_pack)?,
            json,
            "ChangePack created",
        ),
        Command::Pack {
            select,
            delete,
            json,
        } => {
            if select.is_some() && delete.is_some() {
                return Err(DraftError::invalid_config(
                    "draft pack accepts only one of -s or -d",
                ));
            }
            if let Some(reference) = select {
                render_json_or_text(
                    app.pack_select_ref(&cwd, &reference)?,
                    json,
                    "ChangePack selected",
                )
            } else if let Some(reference) = delete {
                let report = app.pack_show(&cwd, &reference)?;
                if !confirm_pack_delete(&report.pack)? {
                    return Err(DraftError::invalid_config("ChangePack deletion aborted"));
                }
                render_json_or_text(
                    app.pack_delete_ref(&cwd, &reference)?,
                    json,
                    "ChangePack deleted",
                )
            } else {
                render_json_or_text(app.pack_show_selected(&cwd)?, json, "ChangePack")
            }
        }
        Command::List { json } => render_json_or_text(app.pack_list(&cwd)?, json, "ChangePacks"),
        Command::Candidate { action } => match action {
            CandidateAction::List { json } => {
                render_json_or_text(app.candidate_list(&cwd)?, json, "Candidates")
            }
            CandidateAction::Show {
                candidate_name,
                json,
            } => render_json_or_text(
                app.candidate_show(&cwd, &candidate_name)?,
                json,
                "Candidate",
            ),
            CandidateAction::Add(args) => render_json_or_text(
                app.candidate_add(
                    &cwd,
                    &args.candidate_name,
                    args.kind.as_deref(),
                    args.template,
                )?,
                args.json,
                "Candidate added",
            ),
            CandidateAction::Update(args) => render_json_or_text(
                app.candidate_update(
                    &cwd,
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
                app.candidate_remove(&cwd, &candidate_name)?,
                json,
                "Candidate removed",
            ),
            CandidateAction::Packs {
                pack,
                candidate,
                json,
            } => render_json_or_text(
                app.candidate_packs(&cwd, pack.as_deref(), candidate.as_deref())?,
                json,
                "Candidate packs",
            ),
        },
        Command::Verify { pack, json } => render_json_or_text(
            app.verify_selected(&cwd, pack.as_deref())?,
            json,
            "Verification complete",
        ),
        Command::Risk {
            pack,
            explain: _,
            include_evidence: _,
            json,
        } => render_json_or_text(
            app.risk_selected(&cwd, pack.as_deref())?,
            json,
            "Risk assessed",
        ),
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
            render_json_or_text(
                app.review_selected(&cwd, pack.as_deref(), comment)?,
                json,
                "Review recorded",
            )
        }
        Command::Approve { pack, reason, json } => render_json_or_text(
            app.decide_selected(&cwd, pack.as_deref(), DecisionKind::Approve, reason)?,
            json,
            "ChangePack approved",
        ),
        Command::Reject { pack, reason, json } => render_json_or_text(
            app.decide_selected(&cwd, pack.as_deref(), DecisionKind::Reject, reason)?,
            json,
            "ChangePack rejected",
        ),
        Command::Compare {
            left,
            right,
            tui,
            json,
        } => {
            if tui {
                return draft_tui::run_review_cockpit(&cwd)
                    .map_err(|e| DraftError::new(DraftErrorKind::Internal, e));
            }
            render_json_or_text(app.compare(&cwd, &left, &right)?, json, "Compare complete")
        }
        Command::Compose {
            left,
            right,
            output: out,
            tui,
            json,
        } => render_json_or_text(
            {
                if tui {
                    return draft_tui::run_review_cockpit(&cwd)
                        .map_err(|e| DraftError::new(DraftErrorKind::Internal, e));
                }
                app.compose(&cwd, &left, &right, &out)?
            },
            json,
            "Compose complete",
        ),
        Command::Disperse {
            pack,
            output,
            tui,
            json,
        } => {
            if tui {
                return draft_tui::run_review_cockpit(&cwd)
                    .map_err(|e| DraftError::new(DraftErrorKind::Internal, e));
            }
            render_json_or_text(
                app.disperse(&cwd, &pack, &output[0], &output[1])?,
                json,
                "Disperse complete",
            )
        }
        Command::Save { pack, vars, json } => {
            let vars = draft_core::parse_hook_vars(vars)?;
            render_json_or_text(
                app.save_selected(&cwd, pack.as_deref(), vars)?,
                json,
                "ChangePack saved",
            )
        }
        Command::Rollback { reference, json } => render_json_or_text(
            app.rollback(&cwd, &reference, true)?,
            json,
            "Rollback complete",
        ),
        Command::Receipt { action } => match action {
            ReceiptAction::List { json } => {
                render_json_or_text(app.receipts(&cwd)?, json, "Receipts")
            }
            ReceiptAction::Show { receipt_id, json } => {
                render_json_or_text(app.receipt_show(&cwd, &receipt_id)?, json, "Receipt")
            }
        },
        Command::Storage { action } => match action {
            StorageAction::Stats { json } => {
                render_json_or_text(app.storage_stats(&cwd)?, json, "Storage stats")
            }
            StorageAction::Gc { json } => {
                render_json_or_text(app.storage_gc(&cwd)?, json, "Storage GC complete")
            }
            StorageAction::Compact { json } => {
                render_json_or_text(app.storage_compact(&cwd)?, json, "Storage compact complete")
            }
            StorageAction::Prune { json } => {
                render_json_or_text(app.storage_prune(&cwd)?, json, "Storage prune complete")
            }
            StorageAction::Doctor { json } => {
                render_json_or_text(app.storage_doctor(&cwd)?, json, "Storage doctor complete")
            }
        },
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
