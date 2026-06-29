use std::io::{self, Write};
use std::path::Path;
use draft_core::errors::DraftError;
use crate::output::print_header;

pub fn run(cwd: &Path, _id: Option<String>, yes: bool) -> Result<(), DraftError> {
    print_header("Undo — Restore checkpoint");
    println!();

    let repo_root = draft_core::repo_detector::RepoDetector::detect(cwd)?.repo_root;
    let storage = draft_core::storage::DraftStorage::open(&repo_root)?;
    let checkpoint = draft_core::checkpoint_engine::CheckpointEngine::latest(&storage)?;

    match checkpoint {
        None => {
            eprintln!("✗ No checkpoint found. Nothing to undo.\n");
            return Err(DraftError::CheckpointMissing);
        }
        Some(cp) => {
            let age = chrono::Utc::now().signed_duration_since(cp.created_at);
            let mins = age.num_minutes();
            let age_str = if mins < 60 {
                format!("{} minutes ago", mins)
            } else {
                format!("{} hours ago", age.num_hours())
            };

            println!("  Checkpoint: {}", &cp.checkpoint_id[..8.min(cp.checkpoint_id.len())]);
            println!("  Created:    {} ({})", cp.created_at.format("%Y-%m-%d %H:%M:%S UTC"), age_str);
            println!("  Message:    {}", cp.message);
            println!();
            println!("  Files that will be restored ({}):", cp.files.len());
            for f in cp.files.iter().take(10) {
                println!("    • {} ({:?})", f.path.display(), f.file_status);
            }
            if cp.files.len() > 10 {
                println!("    ... and {} more", cp.files.len() - 10);
            }
            println!();
            println!("  ⚠  This will overwrite your current working tree files.");

            if !yes {
                print!("  Restore checkpoint? [y/N] ");
                io::stdout().flush().ok();
                let mut input = String::new();
                io::stdin().read_line(&mut input).ok();
                let trimmed = input.trim().to_lowercase();
                if trimmed != "y" && trimmed != "yes" {
                    println!("\n  Undo cancelled.\n");
                    return Ok(());
                }
            }

            let ctx = draft_core::repo_detector::RepoDetector::detect(&repo_root)?;
            let plan = draft_core::checkpoint_engine::CheckpointEngine::restore(
                &ctx, &storage, &cp.checkpoint_id,
            )?;
            draft_core::checkpoint_engine::CheckpointEngine::apply_restore(
                &ctx, &storage, plan,
            )?;

            println!("\n✓ Checkpoint restored successfully.");
            println!("  Working tree reverted to pre-commit state.\n");
        }
    }

    Ok(())
}
