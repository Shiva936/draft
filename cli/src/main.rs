mod output;

use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use draft_core::error::{DraftError, DraftErrorKind};
use draft_core::App;

#[derive(Parser)]
#[command(name = "draft", version = draft_core::DRAFT_VERSION, about = "Draft v0.3.2 - Verified Composable Changepacks")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
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
    /// Launch the local AG-UI Review Cockpit in a browser.
    Cockpit {
        /// Port to bind (loopback only).
        #[arg(long, default_value_t = 4317)]
        port: u16,
    },
    /// Run the MCP adapter (JSON-RPC over stdio) for AI tools.
    Mcp,
    /// ACP adapter: approval workflow operations.
    Acp {
        #[command(subcommand)]
        action: AcpCliAction,
    },
    /// A2A adapter: candidate/actor coordination.
    A2a {
        #[command(subcommand)]
        action: A2aCliAction,
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
    /// Verify a pack: risk + evidence-based test/fuzz selection.
    Verify {
        /// Pack to verify (pck_id or name); enables v0.3.2 evidence verification.
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
        /// Show what would be saved and which checks pass, without saving.
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
enum AcpCliAction {
    /// Show the evidence needed to decide on a pack.
    RequestApproval { pack_id: String },
    /// Approve a pack (emits a signed receipt).
    Approve {
        pack_id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Reject a pack (emits a signed receipt).
    Reject {
        pack_id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// List packs awaiting approval.
    ListPending,
}

#[derive(Subcommand)]
enum A2aCliAction {
    /// Register a candidate in the global registry.
    Register {
        name: String,
        #[arg(long, default_value = "ai")]
        kind: String,
        #[arg(long, default_value = "local")]
        provider: String,
    },
    /// List registered candidates.
    List,
    /// Link a candidate to a pack (provenance only).
    Link { candidate: String, pack_id: String },
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
                DraftErrorKind::SaveFailed => ExitCode::from(8),
                DraftErrorKind::Storage => ExitCode::from(9),
                DraftErrorKind::ConflictDetected => ExitCode::from(2),
                _ => ExitCode::FAILURE,
            }
        }
    }
}

fn run(cli: Cli) -> Result<(), DraftError> {
    let cwd = std::env::current_dir().map_err(DraftError::from)?;
    let app = App::new();
    match cli.command {
        Command::Init { base, global, json } => {
            if global {
                render_init_global(app.init_global()?, json)
            } else {
                render_init(
                    app.init_with_base(&cwd, base.as_deref().unwrap_or("base"))?,
                    json,
                )
            }
        }
        Command::Doctor { global, json } => {
            let report = if global {
                app.doctor_global()?
            } else {
                app.doctor(&cwd)?
            };
            render_doctor(report, json)
        }
        Command::Identity { action } => match action {
            IdentityAction::Status { json } => render_identity(app.identity_status()?, json),
        },
        Command::Cockpit { port } => {
            // Ensure we are inside a workspace before starting the server.
            app.status(&cwd)?;
            draft_agui::serve(cwd.clone(), "127.0.0.1", port)
                .map_err(|e| DraftError::new(DraftErrorKind::Internal, e))
        }
        Command::Mcp => draft_adapters::mcp::serve_stdio(cwd.clone())
            .map_err(|e| DraftError::new(DraftErrorKind::Internal, e)),
        Command::Acp { action } => {
            use draft_adapters::acp::{run, AcpOp};
            let value = match action {
                AcpCliAction::RequestApproval { pack_id } => {
                    run(&cwd, AcpOp::RequestApproval { pack_id: &pack_id })
                }
                AcpCliAction::Approve { pack_id, reason } => run(
                    &cwd,
                    AcpOp::Approve {
                        pack_id: &pack_id,
                        reason,
                    },
                ),
                AcpCliAction::Reject { pack_id, reason } => run(
                    &cwd,
                    AcpOp::Reject {
                        pack_id: &pack_id,
                        reason,
                    },
                ),
                AcpCliAction::ListPending => run(&cwd, AcpOp::ListPending),
            }
            .map_err(|e| DraftError::new(DraftErrorKind::Internal, e))?;
            output::print_json(&value);
            Ok(())
        }
        Command::A2a { action } => {
            use draft_adapters::a2a::{run, A2aOp};
            let value = match action {
                A2aCliAction::Register {
                    name,
                    kind,
                    provider,
                } => run(
                    &cwd,
                    A2aOp::RegisterCandidate {
                        name: &name,
                        kind: &kind,
                        provider: &provider,
                    },
                ),
                A2aCliAction::List => run(&cwd, A2aOp::ListCandidates),
                A2aCliAction::Link { candidate, pack_id } => run(
                    &cwd,
                    A2aOp::Link {
                        candidate: &candidate,
                        pack_id: &pack_id,
                    },
                ),
            }
            .map_err(|e| DraftError::new(DraftErrorKind::Internal, e))?;
            output::print_json(&value);
            Ok(())
        }
        Command::Config { key, action } => match action {
            Some(ConfigAction::Get { key, json }) => {
                render_config(app.config_get_layered(&cwd, &key)?, json)
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
                    app.config_set(&cwd, &key, &value)?
                };
                render_config(report, json)
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
            page,
            limit,
            raw,
            json,
        } => render_events(
            app.events_page(&cwd, false, false, page, limit, None)?,
            json,
            raw,
        ),
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
            algebra: Some(action),
            ..
        } => match action {
            PackAlgebra::Inspect { pack_id, json } => {
                render_json_or_text(app.pack_inspect(&cwd, &pack_id)?, json, "Pack")
            }
            PackAlgebra::Depends { pack_id, json } => {
                render_json_or_text(app.pack_depends(&cwd, &pack_id)?, json, "Dependencies")
            }
            PackAlgebra::Conflicts {
                pack_a,
                pack_b,
                json,
            } => {
                let report = app.pack_conflicts(&cwd, &pack_a, &pack_b)?;
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
                app.pack_compose(&cwd, &pack_a, &pack_b, &name)?,
                json,
                "Pack composed (re-verify required)",
            ),
        },
        Command::Pack {
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
                    app.pack_export(&cwd, &reference, output.as_deref().map(Path::new))?,
                    json,
                    "Pack exported",
                )
            } else if let Some(artifact) = import {
                render_json_or_text(
                    app.pack_import(&cwd, Path::new(&artifact), name.as_deref(), dry_run)?,
                    json,
                    if dry_run {
                        "Import dry run"
                    } else {
                        "Pack imported to quarantine"
                    },
                )
            } else if let Some(reference) = select {
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
        Command::Verify {
            target,
            pack,
            explain,
            full,
            fuzz,
            json,
        } => {
            // v0.3.2 evidence verification triggers on a positional target or an
            // evidence flag; legacy `verify -p <pack>` stays on the legacy path.
            if target.is_some() || explain || full || fuzz {
                let reference = target.or(pack);
                let refstr = reference.as_deref().unwrap_or("selected");
                // Legacy changepacks also need the legacy verification receipt
                // so the legacy save gate passes (same as `verify -p`).
                // Imported packs are canonical-only and skip it.
                if app.is_legacy_pack_ref(&cwd, refstr) {
                    app.verify_selected(&cwd, Some(refstr))?;
                }
                let report = app.verify_pack_v2(&cwd, refstr, full, fuzz)?;
                render_verify_v2(report, explain, json)
            } else {
                let reference = app.resolve_pack_arg(&cwd, pack.as_deref())?;
                render_json_or_text(
                    {
                        let report = app.verify_selected(&cwd, Some(&reference))?;
                        app.verify_pack_v2(&cwd, &reference, false, false)?;
                        report
                    },
                    json,
                    "Verification complete",
                )
            }
        }
        Command::Risk {
            pack,
            explain,
            include_evidence,
            json,
        } => render_json_or_text(
            app.risk_selected_with_options(&cwd, pack.as_deref(), explain, include_evidence)?,
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
            app.cockpit_decide(
                &cwd,
                app.resolve_pack_arg(&cwd, pack.as_deref())?.as_str(),
                true,
                reason,
            )?,
            json,
            "ChangePack approved",
        ),
        Command::Reject { pack, reason, json } => render_json_or_text(
            app.cockpit_decide(
                &cwd,
                app.resolve_pack_arg(&cwd, pack.as_deref())?.as_str(),
                false,
                reason,
            )?,
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
        Command::Save {
            pack,
            vars,
            dry_run,
            json,
        } => {
            if dry_run {
                render_dry_run(app.save_dry_run(&cwd, pack.as_deref())?, json)
            } else {
                let vars = draft_core::parse_hook_vars(vars)?;
                render_json_or_text(
                    app.save_selected(&cwd, pack.as_deref(), vars)?,
                    json,
                    "ChangePack saved",
                )
            }
        }
        Command::Rollback {
            reference,
            dry_run,
            json,
        } => {
            if dry_run {
                render_dry_run(app.rollback_dry_run(&cwd, &reference)?, json)
            } else {
                render_json_or_text(
                    app.rollback(&cwd, &reference, true)?,
                    json,
                    "Rollback complete",
                )
            }
        }
        Command::Receipt { action } => match action {
            ReceiptAction::List { json } => {
                render_json_or_text(app.receipts(&cwd)?, json, "Receipts")
            }
            ReceiptAction::Show { receipt_id, json } => {
                render_json_or_text(app.receipt_show(&cwd, &receipt_id)?, json, "Receipt")
            }
            ReceiptAction::Verify {
                receipt_id,
                all,
                json,
            } => {
                if all {
                    let v = app.receipt_verify_all(&cwd)?;
                    render_ledger_verification(v, json)
                } else if let Some(id) = receipt_id {
                    let v = app.receipt_verify(&cwd, &id)?;
                    render_receipt_verification(v, json)
                } else {
                    Err(DraftError::invalid_config(
                        "provide a receipt id (rcp_...) or --all",
                    ))
                }
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

fn render_verify_v2(
    report: draft_core::VerifyV2Report,
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
