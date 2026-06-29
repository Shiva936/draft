//! Draft CLI — a provider-neutral, service-aware client over `core::App`.
//!
//! v0.2.0 runs in **embedded mode** (talking directly to `core` + the provider
//! registry). When `draftd` is available it is preferred for coordination;
//! safe read-only commands always have an embedded fallback (FR-CLI-003).

mod output;
mod service;

use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use draft_core::app::{App, FinalizeOptions};
use draft_core::error::{DraftError, DraftErrorKind};

#[derive(Parser)]
#[command(name = "draft", version = draft_core::DRAFT_VERSION, about = "Draft — a provider-neutral collaboration workspace before finalization")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a Draft workspace here (alias for `workspace init`).
    Start {
        #[arg(long)]
        json: bool,
    },
    /// Show provider-neutral workspace status.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Review grouped changes (launches the TUI when interactive).
    Review {
        /// Do not launch the interactive UI; print a text summary.
        #[arg(long)]
        no_ui: bool,
        /// Approve all change groups non-interactively.
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    /// Run configured verification commands.
    Verify {
        /// Override the verification command.
        command: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Finalize reviewed changes into a provider-native object (e.g. a commit).
    Commit {
        #[arg(short, long)]
        message: String,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Do not require/attach verification.
        #[arg(long)]
        no_verify: bool,
        /// Proceed even if risk is high/critical.
        #[arg(long)]
        allow_high_risk: bool,
        #[arg(long)]
        json: bool,
    },
    /// Undo the most recent finalization (safe where the provider supports it).
    Undo {
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    /// Create a checkpoint of the current working state.
    Checkpoint {
        #[arg(short, long)]
        message: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Manage the local Draft service (`draftd`).
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// Inspect available providers.
    Provider {
        #[command(subcommand)]
        action: ProviderAction,
    },
    /// Workspace lifecycle.
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },
    /// Inspect durable receipts.
    Receipt {
        #[command(subcommand)]
        action: ReceiptAction,
    },
}

#[derive(Subcommand)]
enum ServiceAction {
    Start,
    Stop,
    Status {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ProviderAction {
    /// List all registered providers and their capabilities.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show the bound provider's status for this workspace.
    Status {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum WorkspaceAction {
    /// Create `.draft/` and bind a provider.
    Init {
        #[arg(long)]
        provider: Option<String>,
        /// Acknowledge using an experimental provider.
        #[arg(long)]
        experimental: bool,
        #[arg(long)]
        json: bool,
    },
    /// Detect which provider owns this path.
    Detect {
        #[arg(long)]
        json: bool,
    },
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", output::format_error(&e));
            match e.kind {
                DraftErrorKind::VerificationFailed
                | DraftErrorKind::RiskPolicyBlocked
                | DraftErrorKind::ReviewRequired
                | DraftErrorKind::ConflictDetected => ExitCode::from(2),
                _ => ExitCode::FAILURE,
            }
        }
    }
}

fn run(cli: Cli) -> Result<(), DraftError> {
    let cwd = std::env::current_dir().map_err(DraftError::from)?;
    let app = App::new(draft_providers::default_registry());

    match cli.command {
        Command::Start { json } => cmd_init(&app, &cwd, None, false, json, true),
        Command::Status { json } => cmd_status(&app, &cwd, json),
        Command::Review { no_ui, yes, json } => cmd_review(&app, &cwd, no_ui, yes, json),
        Command::Verify { command, json } => cmd_verify(&app, &cwd, command, json),
        Command::Commit {
            message,
            yes,
            no_verify,
            allow_high_risk,
            json,
        } => cmd_commit(&app, &cwd, message, yes, no_verify, allow_high_risk, json),
        Command::Undo { yes, json } => cmd_undo(&app, &cwd, yes, json),
        Command::Checkpoint { message, json } => cmd_checkpoint(&app, &cwd, message, json),
        Command::Service { action } => service::handle(action, &cwd),
        Command::Provider { action } => cmd_provider(&app, &cwd, action),
        Command::Workspace { action } => match action {
            WorkspaceAction::Init {
                provider,
                experimental,
                json,
            } => cmd_init(
                &app,
                &cwd,
                provider.map(draft_core::vcs::types::ProviderId::new),
                experimental,
                json,
                false,
            ),
            WorkspaceAction::Detect { json } => cmd_detect(&app, &cwd, json),
        },
        Command::Receipt { action } => cmd_receipt(&app, &cwd, action),
    }
}

fn cmd_init(
    app: &App,
    cwd: &Path,
    provider: Option<draft_core::vcs::types::ProviderId>,
    experimental: bool,
    json: bool,
    started: bool,
) -> Result<(), DraftError> {
    let report = app.init(cwd, provider, experimental)?;
    if json {
        output::print_json(&report);
        return Ok(());
    }
    if report.created {
        output::success(&format!(
            "Initialized Draft workspace ({}) with provider '{}'.",
            report.workspace_id, report.provider_id
        ));
    } else {
        output::warn("Draft workspace already initialized here.");
    }
    output::field("Root", &report.root);
    output::field(
        ".draft excluded",
        if report.draft_excluded { "yes" } else { "no" },
    );
    if started {
        println!("\nRun `draft status` to see changes.");
    }
    Ok(())
}

fn cmd_status(app: &App, cwd: &Path, json: bool) -> Result<(), DraftError> {
    // Prefer the service when it is running; fall back to embedded core.
    if let Some(result) = service::try_ipc(
        "workspace.status",
        serde_json::json!({ "path": cwd.display().to_string() }),
    ) {
        if let Ok(r) = serde_json::from_value::<draft_core::app::StatusReport>(result) {
            return render_status(&r, json);
        }
    }
    let r = app.status(cwd)?;
    render_status(&r, json)
}

fn render_status(r: &draft_core::app::StatusReport, json: bool) -> Result<(), DraftError> {
    if json {
        output::print_json(&r);
        return Ok(());
    }
    output::header("Workspace status");
    output::field("Workspace", &r.workspace_id);
    output::field(
        "Provider",
        &format!("{} ({})", r.provider_id, r.provider_view),
    );
    output::field(
        "Changes",
        &format!(
            "{} file(s), +{} -{}",
            r.changed_files, r.additions, r.deletions
        ),
    );
    for g in &r.change_groups {
        println!(
            "    - {} [{}] ({} files, {})",
            g.title, g.id, g.files, g.review_state
        );
    }
    output::field(
        "Risk",
        &format!("{} ({} finding(s))", r.risk_level, r.risk_findings),
    );
    output::field(
        "Verification",
        r.verification_status.as_deref().unwrap_or("not run"),
    );
    if r.conflicts > 0 {
        output::warn(&format!("{} conflict(s) detected", r.conflicts));
    }
    if let Some(rc) = &r.last_receipt {
        output::field("Last receipt", rc);
    }
    Ok(())
}

fn cmd_review(app: &App, cwd: &Path, no_ui: bool, yes: bool, json: bool) -> Result<(), DraftError> {
    if !no_ui && !yes && !json && std::io::stdout().is_terminal() {
        return draft_tui::run(app, cwd);
    }
    let r = app.review(cwd, yes)?;
    if json {
        output::print_json(&r);
        return Ok(());
    }
    output::header("Review");
    output::field("Session", &r.session_id);
    for g in &r.change_groups {
        println!(
            "  - {} [{}] {} file(s) — {}",
            g.title, g.id, g.files, g.review_state
        );
    }
    if yes {
        output::success(&format!(
            "Approved {} change group(s).",
            r.change_groups.len()
        ));
    } else {
        println!("\nApprove with `draft review --yes`, then `draft commit -m \"...\"`.");
    }
    Ok(())
}

fn cmd_verify(
    app: &App,
    cwd: &Path,
    command: Option<String>,
    json: bool,
) -> Result<(), DraftError> {
    let r = app.verify(cwd, command)?;
    if json {
        output::print_json(&r);
    } else {
        output::header("Verification");
        for c in &r.commands {
            println!("  $ {}  → {}", c.command, c.status);
        }
        output::field("Result", &r.status);
    }
    if r.status != "passed" && r.status != "skipped" {
        return Err(DraftError::new(
            DraftErrorKind::VerificationFailed,
            format!("verification {}", r.status),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_commit(
    app: &App,
    cwd: &Path,
    message: String,
    _yes: bool,
    no_verify: bool,
    allow_high_risk: bool,
    json: bool,
) -> Result<(), DraftError> {
    let r = app.finalize(
        cwd,
        FinalizeOptions {
            message,
            trailers: vec![],
            no_verify,
            confirm_high_risk: allow_high_risk,
        },
    )?;
    if json {
        output::print_json(&r);
        return Ok(());
    }
    for w in &r.warnings {
        output::warn(w);
    }
    let label = r
        .provider_object_label
        .as_deref()
        .unwrap_or(&r.provider_object);
    output::success(&format!(
        "Finalized {} Draft change(s) into {} {}.",
        r.change_count, r.provider_object_kind, label
    ));
    output::field("Receipt", &r.receipt_id);
    Ok(())
}

fn cmd_undo(app: &App, cwd: &Path, _yes: bool, json: bool) -> Result<(), DraftError> {
    let r = app.undo(cwd)?;
    if json {
        output::print_json(&r);
        return Ok(());
    }
    if r.undone {
        output::success(&r.message);
        output::field("Receipt", &r.receipt_id);
    } else {
        output::warn(&r.message);
    }
    Ok(())
}

fn cmd_checkpoint(
    app: &App,
    cwd: &Path,
    message: Option<String>,
    json: bool,
) -> Result<(), DraftError> {
    let r = app.checkpoint(cwd, message)?;
    if json {
        output::print_json(&r);
    } else {
        output::success(&format!("Created checkpoint {}.", r.checkpoint_id));
    }
    Ok(())
}

fn cmd_provider(app: &App, cwd: &Path, action: ProviderAction) -> Result<(), DraftError> {
    match action {
        ProviderAction::List { json } => {
            let providers = app.providers();
            if json {
                output::print_json(&providers);
                return Ok(());
            }
            output::header("Providers");
            for p in providers {
                let tag = if p.experimental {
                    " (experimental)"
                } else {
                    ""
                };
                println!("  {:<10} {}{}", p.id, p.name, tag);
                println!("    {}", p.description);
                if !p.capabilities.is_empty() {
                    println!("    capabilities: {}", p.capabilities.join(", "));
                }
            }
            Ok(())
        }
        ProviderAction::Status { json } => {
            let r = app.provider_status(cwd)?;
            if json {
                output::print_json(&r);
            } else {
                output::header("Provider status");
                output::field(
                    "Provider",
                    &format!("{} ({})", r.provider_id, r.provider_name),
                );
                output::field("Root", &r.root);
                output::field("State", &r.reason);
                output::field("Capabilities", &r.capabilities.join(", "));
                if r.experimental {
                    output::warn("This provider is experimental.");
                }
            }
            Ok(())
        }
    }
}

fn cmd_detect(app: &App, cwd: &Path, json: bool) -> Result<(), DraftError> {
    let r = app.detect(cwd)?;
    if json {
        output::print_json(&r);
    } else {
        output::header("Provider detection");
        output::field(
            "Provider",
            &format!("{} ({})", r.provider_id, r.provider_name),
        );
        output::field("Root", &r.root);
        output::field("Confidence", &r.confidence);
        output::field("Reason", &r.reason);
        if r.experimental {
            output::warn("Detected provider is experimental.");
        }
    }
    Ok(())
}

fn cmd_receipt(app: &App, cwd: &Path, action: ReceiptAction) -> Result<(), DraftError> {
    match action {
        ReceiptAction::List { json } => {
            let receipts = app.receipt_list(cwd)?;
            if json {
                output::print_json(&receipts);
            } else {
                output::header("Receipts");
                if receipts.is_empty() {
                    println!("  (none yet)");
                }
                for r in receipts {
                    println!(
                        "  {}  {}  {} object(s)",
                        r.id,
                        r.created_at.to_rfc3339(),
                        r.provider_objects.len()
                    );
                }
            }
            Ok(())
        }
        ReceiptAction::Show { receipt_id, json } => {
            let r = app.receipt_show(cwd, &receipt_id)?;
            if json {
                output::print_json(&r);
            } else {
                print!("{}", r.render());
            }
            Ok(())
        }
    }
}
