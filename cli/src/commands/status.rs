use std::path::Path;
use draft_core::errors::DraftError;
use draft_core::models::VerificationStatus;
use crate::output::{risk_label, print_header};

pub fn run(cwd: &Path, json: bool) -> Result<(), DraftError> {
    let result = draft_core::get_status(cwd)?;

    if json {
        let json_str = serde_json::to_string_pretty(&result)
            .map_err(|e| DraftError::StorageError(e.to_string()))?;
        println!("{}", json_str);
        return Ok(());
    }

    print_header("Draft status");
    println!();

    let repo_name = result.repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    println!("  Repo:           {}", repo_name);
    
    if let Some(branch) = &result.branch {
        println!("  Branch:         {}", branch);
    }

    let head_short = if result.head.len() >= 7 { &result.head[..7] } else { &result.head };
    println!("  HEAD:           {}", head_short);
    println!("  Changed files:  {}", result.changed_files);
    
    let risk_str = risk_label(result.risk_summary.level);
    println!("  Risk:           {}", risk_str);

    let verification_str = match &result.verification {
        Some(ev) => match ev.status {
            VerificationStatus::Passed => format!("passed ({})", ev.command),
            VerificationStatus::Failed => format!("failed ({})", ev.command),
            _ => "unknown".to_string(),
        },
        None => "not run".to_string(),
    };
    println!("  Verification:   {}", verification_str);

    let checkpoint_str = match &result.last_checkpoint {
        Some(cp) => {
            let age = chrono::Utc::now().signed_duration_since(cp.created_at);
            let mins = age.num_minutes();
            if mins < 60 {
                format!("{} minutes ago", mins)
            } else {
                format!("{} hours ago", age.num_hours())
            }
        }
        None => "none".to_string(),
    };
    println!("  Last checkpoint: {}", checkpoint_str);

    println!();

    if result.changed_files == 0 {
        println!("  Working tree is clean. Nothing to review.");
    } else {
        println!("  Next:");
        println!("    draft review");
    }

    println!();
    Ok(())
}
