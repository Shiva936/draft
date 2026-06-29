use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use draft_core::models::{VerificationEvidence, VerificationStatus};

pub fn append_lines(lines: &mut Vec<Line<'_>>, verification: &Option<VerificationEvidence>) {
    lines.push(Line::from(Span::styled(
        "Verification:",
        Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan),
    )));

    match verification {
        Some(ev) => {
            let (label, color) = match ev.status {
                VerificationStatus::Passed => ("PASSED", Color::Green),
                VerificationStatus::Failed => ("FAILED", Color::Red),
                _ => ("UNKNOWN", Color::Yellow),
            };
            lines.push(Line::from(vec![
                Span::raw("  Command: "),
                Span::styled(ev.command.clone(), Style::default().fg(Color::Cyan)),
            ]));
            lines.push(Line::from(vec![
                Span::raw("  Status:  "),
                Span::styled(label, Style::default().fg(color).add_modifier(Modifier::BOLD)),
                Span::raw(format!("  ({} ms)", ev.duration_ms)),
            ]));
        }
        None => {
            lines.push(Line::raw("  Not run. Press 'T' to verify."));
        }
    }
}
