use draft_core::errors::DraftError;
use draft_core::models::RiskLevel;

/// Format a DraftError into a user-friendly error string
pub fn format_error(err: &DraftError) -> String {
    let msg = err.to_string();
    format!("✗ Error: {}\n\nRun 'draft --help' for usage.", msg)
}

/// Format a risk level into a colored string indicator
pub fn risk_label(level: RiskLevel) -> &'static str {
    match level {
        RiskLevel::Low => "low",
        RiskLevel::Medium => "medium",
        RiskLevel::High => "high",
        RiskLevel::Blocked => "BLOCKED",
    }
}

/// Print a styled header line
pub fn print_header(title: &str) {
    println!("\n{}", title);
    println!("{}", "─".repeat(title.len().min(60)));
}

/// Print a success message
pub fn print_success(msg: &str) {
    println!("✓ {}", msg);
}

/// Print a warning message
pub fn print_warning(msg: &str) {
    eprintln!("⚠ Warning: {}", msg);
}
