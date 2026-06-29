use std::io::{self, Write};
use std::path::Path;
use draft_core::errors::DraftError;
use draft_core::models::{RiskLevel, VerificationStatus};
use crate::output::{risk_label, print_header};

pub fn run(
    cwd: &Path,
    message: String,
    yes: bool,
    no_verify: bool,
    json: bool,
) -> Result<(), DraftError> {
    // Get current review state
    let review = draft_core::review_repo(cwd)?;

    if review.groups.is_empty() {
        eprintln!("\n✗ No changes to commit. Working tree is clean.\n");
        return Err(DraftError::CommitBlocked("Nothing to commit.".to_string()));
    }

    // Show commit plan summary
    if !json {
        print_header("Commit plan");
        println!();
        println!("  Message: {}", message);
        println!();

        let included_files: Vec<_> = review.groups.iter()
            .filter(|g| g.included)
            .flat_map(|g| &g.files)
            .collect();
        let excluded_files: Vec<_> = review.groups.iter()
            .filter(|g| !g.included)
            .flat_map(|g| &g.files)
            .collect();

        println!("  Including {} file(s):", included_files.len());
        for f in included_files.iter().take(10) {
            println!("    + {}", f.display());
        }
        if included_files.len() > 10 {
            println!("    ... and {} more", included_files.len() - 10);
        }

        if !excluded_files.is_empty() {
            println!();
            println!("  Excluding {} file(s) (will remain uncommitted):", excluded_files.len());
            for f in excluded_files.iter().take(5) {
                println!("    - {}", f.display());
            }
        }

        println!();
        println!("  Overall risk: {}", risk_label(review.risk_summary.level));

        // Verification status
        match &review.verification {
            Some(ev) => {
                let status_str = match ev.status {
                    VerificationStatus::Passed => "✓ Passed",
                    VerificationStatus::Failed => "✗ Failed",
                    _ => "? Unknown",
                };
                println!("  Verification: {} ({})", status_str, ev.command);
            }
            None => {
                println!("  Verification: not run");
                if review.risk_summary.level >= RiskLevel::Medium {
                    println!("  ⚠  Consider running `draft verify` before committing.");
                }
            }
        }

        println!();

        // Safety warnings
        if review.risk_summary.level == RiskLevel::Blocked {
            eprintln!("✗ Commit blocked: unresolved conflicts detected.");
            eprintln!("  Resolve conflicts first, then re-run draft review.\n");
            return Err(DraftError::CommitBlocked(
                "Unresolved conflicts block commit.".to_string(),
            ));
        }

        if review.risk_summary.level == RiskLevel::High && !yes {
            println!("  ⚠  High-risk changes detected. Recommended to run `draft review` first.");
            println!();
        }

        // Ask confirmation unless --yes
        if !yes {
            print!("  Create commit? [y/N] ");
            io::stdout().flush().ok();
            let mut input = String::new();
            io::stdin().read_line(&mut input).ok();
            let trimmed = input.trim().to_lowercase();
            if trimmed != "y" && trimmed != "yes" {
                println!("\n  Commit cancelled.\n");
                return Ok(());
            }
        }
    }

    // Execute commit
    println!("\n  Creating checkpoint...");
    
    let req = draft_core::CommitRequest {
        message: message.clone(),
        groups: review.groups,
        no_verify,
    };

    let result = draft_core::create_commit(cwd, req)?;

    if json {
        let json_str = serde_json::to_string_pretty(&result)
            .map_err(|e| DraftError::StorageError(e.to_string()))?;
        println!("{}", json_str);
        return Ok(());
    }

    let hash_short = if result.commit_hash.len() >= 7 {
        &result.commit_hash[..7]
    } else {
        &result.commit_hash
    };

    println!("\n✓ Commit created: {}", hash_short);
    println!();
    println!("  Included:  {} file(s)", result.receipt.included_files.len());
    println!("  Excluded:  {} file(s)", result.receipt.excluded_files.len());
    
    if let Some(ev) = &result.receipt.verification {
        println!("  Verified:  {} ({})", ev.command, match ev.status {
            VerificationStatus::Passed => "passed",
            VerificationStatus::Failed => "failed",
            _ => "unknown",
        });
    }
    
    println!();
    println!("  Receipt:   .draft/receipts/{}.json", result.commit_hash);
    println!();
    println!("  To undo this commit:");
    println!("    draft undo");
    println!();

    Ok(())
}
