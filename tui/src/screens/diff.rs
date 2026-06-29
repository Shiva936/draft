use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::state::TuiState;
use crate::widgets::diff_view;

pub fn draw_diff_panel(f: &mut Frame, rect: Rect, state: &TuiState) {
    if state.groups.is_empty() {
        f.render_widget(
            Paragraph::new("No diff to show.")
                .block(Block::default().title(" Diff ").borders(Borders::ALL)),
            rect,
        );
        return;
    }

    let group = &state.groups[state.selected_group_index];
    let title = format!(" Diff: {} ", group.title);
    let scroll_info = format!(" ({}/{}) ", state.diff_scroll_y + 1, state.diff_lines_cache.len());

    let block = Block::default()
        .title(title)
        .title(Span::styled(scroll_info, Style::default().fg(Color::Cyan)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    let height = rect.height as usize - 2;
    let lines: Vec<Line> = state.diff_lines_cache
        .iter()
        .skip(state.diff_scroll_y)
        .take(height)
        .map(|l| diff_view::colorize_line(l))
        .collect();

    f.render_widget(Paragraph::new(lines).block(block), rect);
}
