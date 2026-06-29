use ratatui::{
    layout::{Constraint, Direction, Layout},
    Frame,
};

use crate::state::{TuiMode, TuiState};

pub mod review;
pub mod diff;
pub mod verify;
pub mod commit;

pub fn draw_ui(f: &mut Frame, state: &mut TuiState) {
    let size = f.size();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(size);

    review::draw_header(f, chunks[0], state);

    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(chunks[1]);

    review::draw_group_list(f, body_chunks[0], state);

    match state.mode {
        TuiMode::Diff => diff::draw_diff_panel(f, body_chunks[1], state),
        TuiMode::Verify => verify::draw_verify_panel(f, body_chunks[1], state),
        _ => review::draw_detail_panel(f, body_chunks[1], state),
    }

    review::draw_footer(f, chunks[2], state);

    if state.mode == TuiMode::CommitConfirm {
        commit::draw_commit_popup(f, size, state);
    }

    if let Some(ref err) = state.error_message.clone() {
        commit::draw_error_popup(f, size, err);
    }

    if let Some(ref hash) = state.commit_success.clone() {
        commit::draw_success_popup(f, size, hash);
    }
}
