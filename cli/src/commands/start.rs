use std::path::Path;
use draft_core::errors::DraftError;

pub fn run(cwd: &Path) -> Result<(), DraftError> {
    let result = draft_core::start_repo(cwd)?;

    println!("\nDraft is ready.\n");
    
    let repo_name = result.repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    println!("  Repo:   {}", repo_name);
    
    if let Some(branch) = &result.branch {
        println!("  Branch: {}", branch);
    } else {
        println!("  Branch: (detached HEAD)");
    }
    
    let head_short = if result.head.len() >= 7 { &result.head[..7] } else { &result.head };
    println!("  HEAD:   {}", head_short);

    if let Some(identity) = &result.identity {
        println!("\n  Git identity:");
        println!("    {} <{}>", identity.name, identity.email);
    } else {
        println!("\n  ⚠  Git identity not configured. Run:");
        println!("    git config user.name \"Your Name\"");
        println!("    git config user.email \"you@example.com\"");
    }

    println!("\n  Safe local store:");
    println!("    {}/.draft/", result.repo_root.display());

    if result.is_new {
        println!("\n  .draft/ is excluded from Git tracking.");
    }

    if result.draft_tracked {
        println!();
        println!("  ⚠  WARNING: .draft/ is currently tracked by Git.");
        println!("     Run: git rm -r --cached .draft/ && git commit -m \"Remove .draft from tracking\"");
    }

    println!("\nKeep coding normally.");
    println!("Run `draft review` before committing.\n");

    Ok(())
}
