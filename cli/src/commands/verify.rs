use std::path::Path;
use draft_core::errors::DraftError;
use draft_core::models::VerificationStatus;
use draft_core::verification_engine::VerificationEngine;
use crate::output::print_header;

pub fn run(cwd: &Path, command: Option<String>, json: bool) -> Result<(), DraftError> {
    // Show the inferred or provided command before running
    let inferred_cmd = match &command {
        Some(cmd) => {
            println!("\nRunning verification: {}\n", cmd);
            cmd.clone()
        }
        None => {
            let inferred = VerificationEngine::infer_command(cwd);
            match inferred {
                Some(cmd) => {
                    println!("\nInferred verification command: {}", cmd);
                    println!("Running...\n");
                    cmd
                }
                None => {
                    eprintln!("\n✗ No verification command provided and none could be inferred.");
                    eprintln!("  Usage: draft verify \"cargo test\"");
                    eprintln!("  Or add a Cargo.toml, go.mod, package.json, pyproject.toml, or Makefile\n");
                    return Err(DraftError::VerificationFailed(
                        "No verification command available.".to_string(),
                    ));
                }
            }
        }
    };

    let evidence = draft_core::run_verification(cwd, Some(inferred_cmd))?;

    if json {
        let json_str = serde_json::to_string_pretty(&evidence)
            .map_err(|e| DraftError::StorageError(e.to_string()))?;
        println!("{}", json_str);
        return Ok(());
    }

    print_header("Verification Result");
    println!();
    println!("  Command:   {}", evidence.command);
    println!("  Duration:  {} ms", evidence.duration_ms);

    match evidence.status {
        VerificationStatus::Passed => {
            println!("  Status:    ✓ PASSED");
            println!("\n  Evidence saved to .draft/verification/");
            println!("  Run `draft commit` when ready.\n");
        }
        VerificationStatus::Failed => {
            println!("  Status:    ✗ FAILED");
            if !evidence.stderr_summary.trim().is_empty() {
                println!("\n  Stderr output:");
                for line in evidence.stderr_summary.lines().take(15) {
                    println!("    {}", line);
                }
            }
            if !evidence.stdout_summary.trim().is_empty() {
                println!("\n  Stdout output (last 10 lines):");
                let lines: Vec<&str> = evidence.stdout_summary.lines().collect();
                let start = lines.len().saturating_sub(10);
                for line in &lines[start..] {
                    println!("    {}", line);
                }
            }
            println!("\n  Evidence saved to .draft/verification/");
            println!("  Fix the failures before committing.\n");
        }
        _ => {
            println!("  Status:    ? Unknown");
        }
    }

    if let Some(exit_code) = evidence.exit_code {
        if exit_code != 0 {
            std::process::exit(exit_code);
        }
    }

    Ok(())
}
