use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

pub fn colorize_line(line: &str) -> Line<'_> {
    let style = if line.starts_with('+') && !line.starts_with("+++") {
        Style::default().fg(Color::Green)
    } else if line.starts_with('-') && !line.starts_with("---") {
        Style::default().fg(Color::Red)
    } else if line.starts_with("@@ ") {
        Style::default().fg(Color::Cyan)
    } else if line.starts_with("diff ") || line.starts_with("index ")
        || line.starts_with("--- ") || line.starts_with("+++ ")
    {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::DIM)
    } else {
        Style::default()
    };
    Line::from(Span::styled(line, style))
}
