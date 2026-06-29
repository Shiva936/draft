use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::state::TuiState;

pub fn draw_commit_popup(f: &mut Frame, size: Rect, state: &TuiState) {
    let popup = centered_rect(size, 60, 50);
    f.render_widget(Clear, popup);

    let outer = Block::default()
        .title(" Commit ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green));

    let inner = outer.inner(popup);
    f.render_widget(outer, popup);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(4), Constraint::Length(2)])
        .split(inner);

    // Message input
    let input_border = if state.commit_input_focused { Color::Cyan } else { Color::DarkGray };
    let input_block = Block::default()
        .title(" Commit Message ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(input_border));
    f.render_widget(Paragraph::new(state.commit_message.as_str()).block(input_block), chunks[0]);

    // Summary
    let included: Vec<_> = state.groups.iter().filter(|g| g.included).flat_map(|g| &g.files).collect();
    let excluded: Vec<_> = state.groups.iter().filter(|g| !g.included).flat_map(|g| &g.files).collect();

    let mut summary: Vec<Line> = Vec::new();
    summary.push(Line::from(Span::styled(
        format!("Include {} file(s):", included.len()),
        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
    )));
    for f_path in included.iter().take(5) {
        summary.push(Line::from(format!("  + {}", f_path.display())));
    }
    if included.len() > 5 {
        summary.push(Line::from(format!("  ... and {} more", included.len() - 5)));
    }
    if !excluded.is_empty() {
        summary.push(Line::raw(""));
        summary.push(Line::from(Span::styled(
            format!("Exclude {} file(s):", excluded.len()),
            Style::default().fg(Color::DarkGray),
        )));
    }
    f.render_widget(Paragraph::new(summary).wrap(Wrap { trim: true }), chunks[1]);

    // Instructions
    let hint = if state.commit_input_focused {
        "Tab/Enter: confirm message  |  Esc: cancel"
    } else {
        "Y: execute commit  |  Tab: edit message  |  Esc: back"
    };
    f.render_widget(
        Paragraph::new(hint).alignment(ratatui::layout::Alignment::Center),
        chunks[2],
    );
}

pub fn draw_error_popup(f: &mut Frame, size: Rect, error: &str) {
    let popup = centered_rect(size, 50, 30);
    f.render_widget(Clear, popup);
    let block = Block::default()
        .title(" Error ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));
    f.render_widget(
        Paragraph::new(format!("{}\n\nPress any key to dismiss.", error))
            .block(block).wrap(Wrap { trim: true })
            .alignment(ratatui::layout::Alignment::Center),
        popup,
    );
}

pub fn draw_success_popup(f: &mut Frame, size: Rect, commit_hash: &str) {
    let popup = centered_rect(size, 50, 30);
    f.render_widget(Clear, popup);
    let block = Block::default()
        .title(" Committed ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green));
    f.render_widget(
        Paragraph::new(format!(
            "Commit created.\n\nHash: {}\n\nReceipt: .draft/receipts/\n\nPress Enter to exit.",
            commit_hash
        ))
        .block(block).wrap(Wrap { trim: true })
        .alignment(ratatui::layout::Alignment::Center),
        popup,
    );
}

fn centered_rect(size: Rect, width_pct: u16, height_pct: u16) -> Rect {
    let vchunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height_pct) / 2),
            Constraint::Percentage(height_pct),
            Constraint::Percentage((100 - height_pct) / 2),
        ])
        .split(size);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width_pct) / 2),
            Constraint::Percentage(width_pct),
            Constraint::Percentage((100 - width_pct) / 2),
        ])
        .split(vchunks[1])[1]
}
