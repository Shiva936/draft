use std::path::PathBuf;
use clap::{Parser, Subcommand};

pub mod commands;
pub mod output;

use commands::{start, status, review, verify, commit, undo};

#[derive(Parser)]
#[command(
    name = "draft",
    version = "0.1.0",
    about = "Draft — Pre-commit trust layer for human + AI code changes",
    long_about = "Draft helps developers safely review, verify, compose, and commit AI-assisted code changes before they enter Git history.\n\nTrust your code before you commit."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize or resume Draft for the current Git repository
    Start,

    /// Show current Draft and Git working tree status
    Status {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Analyze current uncommitted changes and open review mode
    Review {
        /// Print text summary instead of launching TUI
        #[arg(long)]
        no_ui: bool,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Run and record verification commands
    Verify {
        /// Explicit verification command to run (e.g. "cargo test")
        command: Option<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Safely create a Git commit from reviewed and accepted changes
    Commit {
        /// Commit message
        #[arg(short = 'm', long)]
        message: String,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,

        /// Skip verification check
        #[arg(long)]
        no_verify: bool,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Recover from the last Draft operation by restoring a checkpoint
    Undo {
        /// Optional checkpoint or receipt ID to restore
        id: Option<String>,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let result = match cli.command {
        Commands::Start => start::run(&cwd),
        Commands::Status { json } => status::run(&cwd, json),
        Commands::Review { no_ui, json } => review::run(&cwd, no_ui, json),
        Commands::Verify { command, json } => verify::run(&cwd, command, json),
        Commands::Commit { message, yes, no_verify, json } => commit::run(&cwd, message, yes, no_verify, json),
        Commands::Undo { id, yes } => undo::run(&cwd, id, yes),
    };

    if let Err(e) = result {
        eprintln!("\n{}", output::format_error(&e));
        std::process::exit(1);
    }
}
