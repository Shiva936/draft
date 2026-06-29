use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::state::TuiState;

pub fn draw_verify_panel(f: &mut Frame, rect: Rect, state: &TuiState) {
    let block = Block::default()
        .title(" Verification ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let lines = if state.verification_running {
        vec![
            Line::raw(""),
            Line::from(Span::styled("Running verification...", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
            Line::raw(""),
            Line::raw("Please wait."),
        ]
    } else {
        match &state.verification {
            Some(ev) => {
                let (label, color) = match ev.status {
                    draft_core::models::VerificationStatus::Passed => ("PASSED", Color::Green),
                    draft_core::models::VerificationStatus::Failed => ("FAILED", Color::Red),
                    _ => ("UNKNOWN", Color::Yellow),
                };
                vec![
                    Line::raw(""),
                    Line::from(vec![
                        Span::raw("Command: "),
                        Span::styled(&ev.command, Style::default().fg(Color::Cyan)),
                    ]),
                    Line::from(vec![
                        Span::raw("Status:  "),
                        Span::styled(label, Style::default().fg(color).add_modifier(Modifier::BOLD)),
                        Span::raw(format!("  ({} ms)", ev.duration_ms)),
                    ]),
                ]
            }
            None => vec![
                Line::raw(""),
                Line::raw("No verification run yet."),
                Line::raw(""),
                Line::raw("Press 'T' in review mode to run."),
            ],
        }
    };

    f.render_widget(Paragraph::new(lines).block(block), rect);
}
