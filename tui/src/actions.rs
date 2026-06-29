use crossterm::event::KeyCode;
use ratatui::{backend::Backend, Terminal};

use crate::state::{TuiMode, TuiState};
use crate::screens::draw_ui;

pub fn handle_key<B: Backend>(
    terminal: &mut Terminal<B>,
    state: &mut TuiState,
    key: KeyCode,
) -> Result<(), draft_core::errors::DraftError> {
    // Success popup — only Enter exits
    if state.commit_success.is_some() {
        if key == KeyCode::Enter {
            state.mode = TuiMode::Exit;
        }
        return Ok(());
    }

    // Error popup — any key clears it
    if state.error_message.is_some() {
        state.error_message = None;
        return Ok(());
    }

    match state.mode {
        TuiMode::Review => handle_review(terminal, state, key)?,
        TuiMode::Diff => handle_diff(state, key),
        TuiMode::CommitConfirm => handle_commit(state, key)?,
        _ => {}
    }

    Ok(())
}

fn handle_review<B: Backend>(
    terminal: &mut Terminal<B>,
    state: &mut TuiState,
    key: KeyCode,
) -> Result<(), draft_core::errors::DraftError> {
    match key {
        KeyCode::Char('q') | KeyCode::Esc => state.mode = TuiMode::Exit,
        KeyCode::Char('j') | KeyCode::Down => state.update_selection(true),
        KeyCode::Char('k') | KeyCode::Up => state.update_selection(false),
        KeyCode::Char(' ') => state.toggle_selected_group(),
        KeyCode::Char('d') | KeyCode::Enter => state.mode = TuiMode::Diff,
        KeyCode::Char('c') => {
            state.mode = TuiMode::CommitConfirm;
            state.commit_input_focused = true;
        }
        KeyCode::Char('t') => {
            state.mode = TuiMode::Verify;
            state.verification_running = true;
            terminal.draw(|f| draw_ui(f, state))
                .map_err(|e| draft_core::errors::DraftError::Io(e.to_string()))?;

            match draft_core::run_verification(&state.repo_root, None) {
                Ok(ev) => state.verification = Some(ev),
                Err(e) => state.error_message = Some(e.to_string()),
            }
            state.verification_running = false;
            state.mode = TuiMode::Review;
        }
        _ => {}
    }
    Ok(())
}

fn handle_diff(state: &mut TuiState, key: KeyCode) {
    match key {
        KeyCode::Char('j') | KeyCode::Down => {
            if state.diff_scroll_y + 1 < state.diff_lines_cache.len() {
                state.diff_scroll_y += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if state.diff_scroll_y > 0 {
                state.diff_scroll_y -= 1;
            }
        }
        KeyCode::Char('d') | KeyCode::Esc | KeyCode::Enter => state.mode = TuiMode::Review,
        _ => {}
    }
}

fn handle_commit(
    state: &mut TuiState,
    key: KeyCode,
) -> Result<(), draft_core::errors::DraftError> {
    if state.commit_input_focused {
        match key {
            KeyCode::Char(c) => state.commit_message.push(c),
            KeyCode::Backspace => { state.commit_message.pop(); }
            KeyCode::Tab | KeyCode::Enter => {
                if !state.commit_message.trim().is_empty() {
                    state.commit_input_focused = false;
                }
            }
            KeyCode::Esc => state.mode = TuiMode::Review,
            _ => {}
        }
    } else {
        match key {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let req = draft_core::CommitRequest {
                    message: state.commit_message.clone(),
                    groups: state.groups.clone(),
                    no_verify: false,
                };
                match draft_core::create_commit(&state.repo_root, req) {
                    Ok(res) => state.commit_success = Some(res.commit_hash),
                    Err(e) => {
                        state.error_message = Some(e.to_string());
                        state.mode = TuiMode::Review;
                    }
                }
            }
            KeyCode::Tab => state.commit_input_focused = true,
            KeyCode::Esc => state.mode = TuiMode::Review,
            _ => {}
        }
    }
    Ok(())
}
