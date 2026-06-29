use std::io;
use std::path::Path;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

pub mod state;
pub mod app;
pub mod actions;
pub mod screens;
pub mod widgets;

use crate::state::TuiState;
use crate::app::run_app;

pub fn run_tui(repo_root: &Path, commit_msg: Option<String>) -> Result<(), draft_core::errors::DraftError> {
    let review_res = draft_core::review_repo(repo_root)?;

    enable_raw_mode().map_err(|e| draft_core::errors::DraftError::Io(e.to_string()))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .map_err(|e| draft_core::errors::DraftError::Io(e.to_string()))?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)
        .map_err(|e| draft_core::errors::DraftError::Io(e.to_string()))?;

    let state = TuiState::new(
        repo_root.to_path_buf(),
        review_res.repo_context,
        review_res.groups,
        review_res.verification,
        commit_msg,
    );

    let res = run_app(&mut terminal, state);

    disable_raw_mode().map_err(|e| draft_core::errors::DraftError::Io(e.to_string()))?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)
        .map_err(|e| draft_core::errors::DraftError::Io(e.to_string()))?;
    terminal.show_cursor().map_err(|e| draft_core::errors::DraftError::Io(e.to_string()))?;

    res
}
