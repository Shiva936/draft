//! Draft TUI — a provider-neutral interactive review/compose surface.
//!
//! It consumes the same provider-neutral application surface as the CLI. Status
//! refreshes prefer `draftd` over local IPC when available and fall back to the
//! embedded core API; mutating review/finalization actions remain embedded in
//! v0.2.0 because the daemon exposes only safe read endpoints.

use std::io;
use std::path::Path;

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use draft_core::app::{App, FinalizeOptions, FinalizeReport, ReviewReport, StatusReport};
use draft_core::error::DraftError;

/// Entry point used by the CLI (`draft review` when interactive).
pub fn run(app: &App, path: &Path) -> Result<(), DraftError> {
    let backend = AppBackend { app, path };
    let mut model = Model::load(&backend)?;

    enable_raw_mode().map_err(io_err)?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture).map_err(io_err)?;
    let backend_term = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend_term).map_err(io_err)?;

    let result = event_loop(&mut terminal, &backend, &mut model);

    disable_raw_mode().ok();
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )
    .ok();
    terminal.show_cursor().ok();

    result
}

fn io_err(e: io::Error) -> DraftError {
    DraftError::storage(e.to_string())
}

trait UiBackend {
    fn status(&self) -> Result<StatusReport, DraftError>;
    fn approve_all(&self) -> Result<ReviewReport, DraftError>;
    fn finalize(&self, message: String) -> Result<FinalizeReport, DraftError>;
}

struct AppBackend<'a> {
    app: &'a App,
    path: &'a Path,
}

impl UiBackend for AppBackend<'_> {
    fn status(&self) -> Result<StatusReport, DraftError> {
        if let Some(status) = service_status(self.path) {
            return Ok(status);
        }
        self.app.status(self.path)
    }

    fn approve_all(&self) -> Result<ReviewReport, DraftError> {
        self.app.review(self.path, true)
    }

    fn finalize(&self, message: String) -> Result<FinalizeReport, DraftError> {
        self.app.finalize(
            self.path,
            FinalizeOptions {
                message,
                trailers: vec![],
                no_verify: false,
                confirm_high_risk: false,
            },
        )
    }
}

fn service_status(path: &Path) -> Option<StatusReport> {
    let sock = draft_ipc::socket_path();
    if !draft_ipc::is_running(&sock) {
        return None;
    }
    let resp = draft_ipc::call(
        &sock,
        &draft_ipc::Request::new(
            "tui",
            "workspace.status",
            serde_json::json!({ "path": path.display().to_string() }),
        ),
    )
    .ok()?;
    if !resp.ok {
        return None;
    }
    serde_json::from_value(resp.result?).ok()
}

struct Model {
    status: StatusReport,
    message: String,
    editing: bool,
    info: String,
    receipt: Option<String>,
}

impl Model {
    fn load(backend: &impl UiBackend) -> Result<Self, DraftError> {
        Ok(Model {
            status: backend.status()?,
            message: String::new(),
            editing: false,
            info: "a: approve all   f: finalize   r: refresh   q: quit".to_string(),
            receipt: None,
        })
    }

    fn refresh(&mut self, backend: &impl UiBackend) {
        if let Ok(s) = backend.status() {
            self.status = s;
        }
    }

    fn approve_all(&mut self, backend: &impl UiBackend) {
        match backend.approve_all() {
            Ok(_) => {
                self.info = "Approved all change groups.".to_string();
                self.refresh(backend);
            }
            Err(e) => self.info = format!("approve failed: {}", e.message),
        }
    }

    fn finalize(&mut self, backend: &impl UiBackend) {
        if self.message.trim().is_empty() {
            self.info = "finalization message cannot be empty".to_string();
            return;
        }
        match backend.finalize(self.message.clone()) {
            Ok(r) => {
                self.receipt = Some(r.receipt_id.clone());
                self.info = format!(
                    "Finalized {} change(s) into {} {}. Receipt {}",
                    r.change_count,
                    r.provider_object_kind,
                    r.provider_object_label.unwrap_or(r.provider_object),
                    r.receipt_id
                );
                self.refresh(backend);
            }
            Err(e) => self.info = format!("finalize blocked: {}", e.message),
        }
        self.message.clear();
    }
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    backend: &impl UiBackend,
    model: &mut Model,
) -> Result<(), DraftError> {
    loop {
        terminal.draw(|f| draw(f, model)).map_err(io_err)?;
        if !event::poll(std::time::Duration::from_millis(150)).map_err(io_err)? {
            continue;
        }
        if let Event::Key(key) = event::read().map_err(io_err)? {
            if model.editing {
                match key.code {
                    KeyCode::Enter => {
                        model.editing = false;
                        model.finalize(backend);
                    }
                    KeyCode::Esc => model.editing = false,
                    KeyCode::Backspace => {
                        model.message.pop();
                    }
                    KeyCode::Char(c) => model.message.push(c),
                    _ => {}
                }
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('r') => model.refresh(backend),
                KeyCode::Char('a') => model.approve_all(backend),
                KeyCode::Char('f') => {
                    model.editing = true;
                    model.info = "Enter finalization message, then press Enter.".to_string();
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn draw(f: &mut Frame, model: &Model) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(f.size());

    let s = &model.status;
    let header = vec![
        Line::from(vec![
            Span::styled("Workspace ", Style::default().fg(Color::Cyan)),
            Span::raw(&s.workspace_id),
        ]),
        Line::from(vec![
            Span::styled("Provider  ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{} - {}", s.provider_id, s.provider_view)),
        ]),
        Line::from(vec![
            Span::styled("Changes   ", Style::default().fg(Color::Cyan)),
            Span::raw(format!(
                "{} file(s)  +{} -{}",
                s.changed_files, s.additions, s.deletions
            )),
        ]),
        Line::from(vec![
            Span::styled("Risk      ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{} ({} findings)", s.risk_level, s.risk_findings)),
            Span::raw("   "),
            Span::styled("Verify ", Style::default().fg(Color::Cyan)),
            Span::raw(
                s.verification_status
                    .clone()
                    .unwrap_or_else(|| "not run".into()),
            ),
        ]),
    ];
    f.render_widget(
        Paragraph::new(header).block(Block::default().borders(Borders::ALL).title("Draft")),
        chunks[0],
    );

    let items: Vec<ListItem> = s
        .change_groups
        .iter()
        .map(|g| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:<16}", g.title),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" {} file(s)  ", g.files)),
                Span::styled(g.review_state.clone(), Style::default().fg(Color::Yellow)),
            ]))
        })
        .collect();
    f.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Change groups"),
        ),
        chunks[1],
    );

    let footer = if model.editing {
        Paragraph::new(format!("message> {}", model.message))
            .block(Block::default().borders(Borders::ALL).title("Finalize"))
    } else {
        Paragraph::new(model.info.clone())
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("a approve - f finalize - r refresh - q quit"),
            )
    };
    f.render_widget(footer, chunks[2]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    use draft_core::app::ChangeGroupSummary;
    use draft_core::error::{DraftError, DraftErrorKind};

    struct FakeBackend {
        status: RefCell<StatusReport>,
        approve_calls: RefCell<usize>,
        finalize_calls: RefCell<Vec<String>>,
        finalize_result: RefCell<Result<FinalizeReport, DraftError>>,
    }

    impl FakeBackend {
        fn new() -> Self {
            FakeBackend {
                status: RefCell::new(StatusReport {
                    workspace_id: "ws_test".to_string(),
                    provider_id: "git".to_string(),
                    provider_view: "on branch main".to_string(),
                    changed_files: 1,
                    additions: 2,
                    deletions: 0,
                    change_groups: vec![ChangeGroupSummary {
                        id: "chg_1".to_string(),
                        title: "Source changes".to_string(),
                        files: 1,
                        review_state: "pending".to_string(),
                    }],
                    risk_level: "low".to_string(),
                    risk_findings: 0,
                    verification_status: None,
                    conflicts: 0,
                    last_receipt: None,
                }),
                approve_calls: RefCell::new(0),
                finalize_calls: RefCell::new(Vec::new()),
                finalize_result: RefCell::new(Ok(FinalizeReport {
                    change_count: 1,
                    provider_object: "abc123".to_string(),
                    provider_object_label: Some("abc123".to_string()),
                    provider_object_kind: "commit".to_string(),
                    receipt_id: "rcpt_1".to_string(),
                    warnings: vec![],
                })),
            }
        }
    }

    impl UiBackend for FakeBackend {
        fn status(&self) -> Result<StatusReport, DraftError> {
            Ok(self.status.borrow().clone())
        }

        fn approve_all(&self) -> Result<ReviewReport, DraftError> {
            *self.approve_calls.borrow_mut() += 1;
            self.status.borrow_mut().change_groups[0].review_state = "approved".to_string();
            Ok(ReviewReport {
                session_id: "rev_1".to_string(),
                change_groups: self.status.borrow().change_groups.clone(),
                decisions: 1,
            })
        }

        fn finalize(&self, message: String) -> Result<FinalizeReport, DraftError> {
            self.finalize_calls.borrow_mut().push(message);
            self.finalize_result.borrow().clone()
        }
    }

    #[test]
    fn model_loads_provider_neutral_status() {
        let backend = FakeBackend::new();
        let model = Model::load(&backend).unwrap();
        assert_eq!(model.status.provider_id, "git");
        assert_eq!(model.status.change_groups[0].review_state, "pending");
    }

    #[test]
    fn approve_action_records_decision_and_refreshes_status() {
        let backend = FakeBackend::new();
        let mut model = Model::load(&backend).unwrap();
        model.approve_all(&backend);
        assert_eq!(*backend.approve_calls.borrow(), 1);
        assert_eq!(model.status.change_groups[0].review_state, "approved");
        assert!(model.info.contains("Approved"));
    }

    #[test]
    fn empty_finalization_message_is_rejected() {
        let backend = FakeBackend::new();
        let mut model = Model::load(&backend).unwrap();
        model.message = "   ".to_string();
        model.finalize(&backend);
        assert!(model.info.contains("cannot be empty"));
        assert!(backend.finalize_calls.borrow().is_empty());
    }

    #[test]
    fn successful_finalization_records_receipt_info() {
        let backend = FakeBackend::new();
        let mut model = Model::load(&backend).unwrap();
        model.message = "ship it".to_string();
        model.finalize(&backend);
        assert_eq!(backend.finalize_calls.borrow().as_slice(), ["ship it"]);
        assert_eq!(model.receipt.as_deref(), Some("rcpt_1"));
        assert!(model.info.contains("Receipt rcpt_1"));
    }

    #[test]
    fn finalization_failure_is_displayed() {
        let backend = FakeBackend::new();
        *backend.finalize_result.borrow_mut() = Err(DraftError::new(
            DraftErrorKind::FinalizationFailed,
            "blocked by policy",
        ));
        let mut model = Model::load(&backend).unwrap();
        model.message = "ship it".to_string();
        model.finalize(&backend);
        assert!(model.info.contains("blocked by policy"));
    }
}
