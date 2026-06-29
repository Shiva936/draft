use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};

use draft_core::models::RiskLevel;

pub fn span(level: RiskLevel) -> Span<'static> {
    let (label, color) = label_color(level);
    Span::styled(label, Style::default().fg(color).add_modifier(Modifier::BOLD))
}

pub fn inline_span(level: RiskLevel) -> Span<'static> {
    let (label, color) = label_color(level);
    Span::styled(
        format!(" {} ", label),
        Style::default().bg(color).fg(Color::Black).add_modifier(Modifier::BOLD),
    )
}

pub fn reason_color(level: RiskLevel) -> Color {
    label_color(level).1
}

fn label_color(level: RiskLevel) -> (&'static str, Color) {
    match level {
        RiskLevel::Low => ("LOW", Color::Green),
        RiskLevel::Medium => ("MED", Color::Yellow),
        RiskLevel::High => ("HIGH", Color::Red),
        RiskLevel::Blocked => ("BLOCKED", Color::Magenta),
    }
}
