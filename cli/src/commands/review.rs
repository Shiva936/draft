use std::path::Path;
use draft_core::errors::DraftError;
use draft_core::models::RiskLevel;
use crate::output::{risk_label, print_header};

pub fn run(cwd: &Path, no_ui: bool, json: bool) -> Result<(), DraftError> {
    let result = draft_core::review_repo(cwd)?;

    if json {
        let json_str = serde_json::to_string_pretty(&result)
            .map_err(|e| DraftError::StorageError(e.to_string()))?;
        println!("{}", json_str);
        return Ok(());
    }

    // Check if terminal is interactive — launch TUI unless --no-ui
    let is_interactive = atty_is_stdout();
    
    if is_interactive && !no_ui {
        // Launch TUI
        draft_tui::run_tui(cwd, None)?;
        return Ok(());
    }

    // Text mode output
    print_header("Review before commit");
    println!();

    if result.groups.is_empty() {
        println!("  Working tree is clean. Nothing to review.");
        println!();
        return Ok(());
    }

    let total_files: usize = result.groups.iter().map(|g| g.files.len()).sum();
    println!("  Changed files: {}", total_files);
    
    let overall_risk = risk_label(result.risk_summary.level);
    println!("  Overall risk:  {}", overall_risk);
    println!();

    println!("  Groups:");
    println!("  {:<4} {:<30} {:<10} {:<8}", "#", "Title", "Files", "Risk");
    println!("  {}", "─".repeat(58));

    for (i, group) in result.groups.iter().enumerate() {
        let check = if group.included { "✓" } else { "✗" };
        let risk_str = risk_label(group.risk.level);
        println!("  {}[{}] {:<28} {:<10} {}",
            check,
            i + 1,
            group.title,
            group.files.len(),
            risk_str
        );

        // Show first risk reason
        if let Some(reason) = group.risk.reasons.first() {
            println!("       ↳ {}", reason.message);
        }
    }

    println!();
    
    // High-risk recommendations
    let has_high = result.groups.iter().any(|g| g.risk.level >= RiskLevel::High && g.included);
    let has_unverified = result.verification.is_none();
    
    println!("  Recommended:");
    if has_high {
        println!("    inspect high-risk changes before committing");
    }
    if has_unverified {
        println!("    run `draft verify` to record test evidence");
    }
    if !has_high && !has_unverified {
        println!("    run `draft commit -m \"your message\"` when ready");
    }

    println!();
    Ok(())
}

fn atty_is_stdout() -> bool {
    // Check if stdout is a real terminal
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}
