use std::time::Duration;
use crossterm::event::{self, Event};
use ratatui::{backend::Backend, Terminal};

use crate::state::TuiState;
use crate::screens::draw_ui;
use crate::actions::handle_key;

pub fn run_app<B: Backend>(terminal: &mut Terminal<B>, mut state: TuiState) -> Result<(), draft_core::errors::DraftError> {
    loop {
        terminal.draw(|f| draw_ui(f, &mut state))
            .map_err(|e| draft_core::errors::DraftError::Io(e.to_string()))?;

        if state.mode == crate::state::TuiMode::Exit {
            break;
        }

        if event::poll(Duration::from_millis(100))
            .map_err(|e| draft_core::errors::DraftError::Io(e.to_string()))?
        {
            if let Event::Key(key) = event::read()
                .map_err(|e| draft_core::errors::DraftError::Io(e.to_string()))?
            {
                handle_key(terminal, &mut state, key.code)?;
            }
        }
    }
    Ok(())
}
