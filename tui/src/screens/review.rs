use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::state::{TuiMode, TuiState};
use crate::widgets::{change_list, risk_badge, evidence_panel};

pub fn draw_header(f: &mut Frame, rect: Rect, state: &TuiState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Draft v0.1.0 ");

    let branch = state.repo_context.branch.as_deref().unwrap_or("detached HEAD");
    let head = if state.repo_context.head.len() >= 7 { &state.repo_context.head[..7] } else { "unborn" };
    let identity = state.repo_context.identity.as_ref()
        .map(|i| format!("{} <{}>", i.name, i.email))
        .unwrap_or_else(|| "unknown".to_string());
    let repo = state.repo_context.repo_root.file_name()
        .and_then(|n| n.to_str()).unwrap_or("");

    let text = format!(" {}  |  branch: {}  |  HEAD: {}  |  {}", repo, branch, head, identity);
    let p = Paragraph::new(text).block(block);
    f.render_widget(p, rect);
}

pub fn draw_group_list(f: &mut Frame, rect: Rect, state: &TuiState) {
    change_list::draw(f, rect, state);
}

pub fn draw_detail_panel(f: &mut Frame, rect: Rect, state: &TuiState) {
    if state.groups.is_empty() {
        let block = Block::default().title(" Inspection Panel ").borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));
        f.render_widget(Paragraph::new("No files to inspect.").block(block), rect);
        return;
    }

    let group = &state.groups[state.selected_group_index];
    let block = Block::default()
        .title(format!(" Group: {} ", group.title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));

    let mut lines: Vec<Line> = Vec::new();
    if let Some(desc) = &group.description {
        lines.push(Line::from(vec![
            Span::styled("Description: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(desc),
        ]));
        lines.push(Line::raw(""));
    }

    lines.push(Line::from(Span::styled("Changed Files:", Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan))));
    for file in &group.files {
        lines.push(Line::from(vec![
            Span::styled("  • ", Style::default().fg(Color::Blue)),
            Span::raw(file.to_string_lossy().into_owned()),
        ]));
    }
    lines.push(Line::raw(""));

    lines.push(Line::from(Span::styled("Risk:", Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan))));
    for reason in &group.risk.reasons {
        let color = risk_badge::reason_color(group.risk.level);
        lines.push(Line::from(vec![
            Span::styled("  ⚠ ", Style::default().fg(color)),
            Span::raw(&reason.message),
        ]));
    }
    if group.risk.reasons.is_empty() {
        lines.push(Line::raw("  No specific risks identified."));
    }
    lines.push(Line::raw(""));

    evidence_panel::append_lines(&mut lines, &state.verification);

    f.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: true }), rect);
}

pub fn draw_footer(f: &mut Frame, rect: Rect, state: &TuiState) {
    let block = Block::default().borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let spans = if state.mode == TuiMode::Diff {
        vec![
            Span::styled("↑/↓ k/j", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(": Scroll  |  "),
            Span::styled("Enter/Esc", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(": Back"),
        ]
    } else {
        let risk_span = risk_badge::span(state.risk_summary.level);
        vec![
            Span::styled("Space", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(": Toggle  |  "),
            Span::styled("D", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(": Diff  |  "),
            Span::styled("T", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(": Verify  |  "),
            Span::styled("C", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(": Commit  |  "),
            Span::styled("Q", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(": Quit  |  Risk: "),
            risk_span,
        ]
    };

    f.render_widget(
        Paragraph::new(Line::from(spans)).block(block)
            .alignment(ratatui::layout::Alignment::Center),
        rect,
    );
}
