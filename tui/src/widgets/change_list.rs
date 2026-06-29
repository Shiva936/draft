use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::state::TuiState;
use super::risk_badge;

pub fn draw(f: &mut Frame, rect: Rect, state: &TuiState) {
    let block = Block::default()
        .title(" Change Groups ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));

    if state.groups.is_empty() {
        f.render_widget(
            Paragraph::new("Working tree clean.")
                .block(block)
                .alignment(ratatui::layout::Alignment::Center),
            rect,
        );
        return;
    }

    let items: Vec<ListItem> = state.groups.iter().enumerate().map(|(idx, group)| {
        let is_selected = idx == state.selected_group_index;
        let check = if group.included { "[x]" } else { "[ ]" };
        let check_color = if group.included { Color::Green } else { Color::DarkGray };
        let title_style = if is_selected {
            Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan)
        } else {
            Style::default()
        };
        let cursor = if is_selected { "> " } else { "  " };
        let risk_span = risk_badge::inline_span(group.risk.level);

        let line = Line::from(vec![
            Span::styled(cursor, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{} ", check), Style::default().fg(check_color)),
            Span::styled(format!("{:<25}", group.title), title_style),
            Span::raw(" "),
            risk_span,
        ]);

        let item = ListItem::new(line);
        if is_selected {
            item.style(Style::default().bg(Color::Rgb(30, 30, 46)))
        } else {
            item
        }
    }).collect();

    f.render_widget(List::new(items).block(block), rect);
}
