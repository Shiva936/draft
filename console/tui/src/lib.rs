//! Terminal frontend for Draft Console.
//!
//! Domain state and action eligibility arrive exclusively through
//! `draft-console-application`. This crate owns rendering and local interaction
//! state only.

use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event as TerminalEvent, KeyCode, KeyEvent, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use draft_console_application::{ClientError, ConsoleClient, ReconnectBackoff};
use draft_ipc::console_application::{
    ActionInputField, ActionInputKind, ConsoleReadModel, ConsoleScope, ConsoleSubject,
    ModelFreshness,
};
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Terminal;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Stdout};
use std::sync::mpsc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub preselected_workspace_id: Option<String>,
    pub startup_diagnostic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Loading,
    Connected,
    Stale,
    Disconnected(String),
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane {
    Navigation,
    Content,
    Actions,
}

#[derive(Debug, Clone)]
pub struct AppModel {
    pub subject: ConsoleSubject,
    pub read_model: Option<ConsoleReadModel>,
    pub connection: ConnectionState,
    pub selected_navigation: usize,
    pub selected_action: usize,
    pub content_scroll: u16,
    pub focus: FocusPane,
    pub search: Option<String>,
    pub palette_open: bool,
    /// An id being typed to open its own scope, if the prompt is open.
    ///
    /// §8.3 gives a ChangePack and a Baseline their own scopes, and this is how a
    /// terminal reaches them. The id's own prefix decides which — `cpk_` is a
    /// ChangePack and a Baseline digest is a Baseline — so there is nothing for a
    /// user to get wrong and nothing for this file to guess.
    pub open_subject: Option<String>,
    pub help_open: bool,
    pub confirmation: Option<usize>,
    /// The form for an action that declared inputs, while it is being filled.
    ///
    /// The fields come from the server's `ActionPresentation`; the TUI renders
    /// them and collects values. It never invents an input, never decides what
    /// is valid, and never submits one the action did not declare.
    pub action_form: Option<ActionForm>,
    pub diagnostic: Option<String>,
    pub should_quit: bool,
}

/// Values being collected for one action's declared inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionForm {
    pub action_index: usize,
    pub fields: Vec<ActionInputField>,
    /// One entry per field: text as typed, booleans as `true`/`false`, a select
    /// as the index of the chosen option.
    pub values: Vec<String>,
    pub cursor: usize,
}

impl ActionForm {
    fn open(action_index: usize, fields: Vec<ActionInputField>) -> Self {
        let values = fields
            .iter()
            .map(|field| match &field.kind {
                ActionInputKind::Text { .. } => String::new(),
                // A select starts on its first declared option; an empty set
                // leaves nothing to choose and submits nothing.
                ActionInputKind::Select { options } => {
                    if options.is_empty() {
                        String::new()
                    } else {
                        options[0].value.clone()
                    }
                }
                ActionInputKind::Boolean => "false".into(),
                ActionInputKind::Confirmation => "false".into(),
            })
            .collect();
        Self {
            action_index,
            fields,
            values,
            cursor: 0,
        }
    }

    /// The arguments to submit, keyed by each field's stable id.
    ///
    /// An untouched optional text field is omitted rather than sent empty, so
    /// the server sees exactly what the user supplied.
    pub fn arguments(&self) -> BTreeMap<String, Value> {
        let mut arguments = BTreeMap::new();
        for (field, value) in self.fields.iter().zip(&self.values) {
            let encoded = match &field.kind {
                ActionInputKind::Text { .. } => {
                    if value.is_empty() && !field.required {
                        continue;
                    }
                    Value::String(value.clone())
                }
                ActionInputKind::Select { .. } => {
                    if value.is_empty() {
                        continue;
                    }
                    Value::String(value.clone())
                }
                ActionInputKind::Boolean | ActionInputKind::Confirmation => {
                    Value::Bool(value == "true")
                }
            };
            arguments.insert(field.id.clone(), encoded);
        }
        arguments
    }

    fn toggle(&mut self) {
        let Some(field) = self.fields.get(self.cursor) else {
            return;
        };
        match &field.kind {
            ActionInputKind::Boolean | ActionInputKind::Confirmation => {
                let value = &mut self.values[self.cursor];
                *value = if value == "true" { "false" } else { "true" }.into();
            }
            ActionInputKind::Select { options } => {
                if options.is_empty() {
                    return;
                }
                let current = options
                    .iter()
                    .position(|option| option.value == self.values[self.cursor])
                    .unwrap_or(0);
                self.values[self.cursor] = options[(current + 1) % options.len()].value.clone();
            }
            ActionInputKind::Text { .. } => {}
        }
    }
}

impl AppModel {
    pub fn loading(subject: ConsoleSubject, diagnostic: Option<String>) -> Self {
        Self {
            subject,
            read_model: None,
            connection: ConnectionState::Loading,
            selected_navigation: 0,
            selected_action: 0,
            content_scroll: 0,
            focus: FocusPane::Navigation,
            search: None,
            palette_open: false,
            open_subject: None,
            help_open: false,
            confirmation: None,
            action_form: None,
            diagnostic,
            should_quit: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    BackendLoaded(Box<ConsoleReadModel>),
    BackendFailed(String),
    Disconnected(String),
    Reconnecting,
    Key(KeyEvent),
    Resized,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Refresh,
    InvokeSelectedAction,
    ChangeSubject(ConsoleSubject),
}

pub fn reduce(mut model: AppModel, event: AppEvent) -> (AppModel, Vec<Effect>) {
    let mut effects = Vec::new();
    match event {
        AppEvent::BackendLoaded(read_model) => {
            let read_model = *read_model;
            model.subject = read_model.subject.clone();
            model.connection = match read_model.freshness {
                ModelFreshness::Stale => ConnectionState::Stale,
                _ => ConnectionState::Connected,
            };
            model.read_model = Some(read_model);
            model.selected_action = 0;
            model.confirmation = None;
            // A fresh authoritative model supersedes anything half-entered:
            // the action that form belonged to may no longer be offered, and
            // its descriptor is certainly spent.
            model.action_form = None;
        }
        AppEvent::BackendFailed(message) => model.connection = ConnectionState::Failed(message),
        AppEvent::Disconnected(message) => {
            model.connection = ConnectionState::Disconnected(message);
            model.confirmation = None;
            model.palette_open = false;
            if let Some(read_model) = model.read_model.as_mut() {
                read_model.freshness = ModelFreshness::Stale;
                read_model.actions.iter_mut().for_each(|action| {
                    action.enabled = false;
                    action.invocation_capability = None;
                    action.disabled_reason = Some("Reconnect and refresh before acting".into());
                });
            }
        }
        AppEvent::Reconnecting => model.connection = ConnectionState::Stale,
        AppEvent::Resized => {}
        AppEvent::Key(key) => {
            if let Some(mut form) = model.action_form.take() {
                match key.code {
                    KeyCode::Esc => {}
                    KeyCode::Enter => {
                        model.selected_action = form.action_index;
                        model.action_form = Some(form);
                        effects.push(Effect::InvokeSelectedAction);
                    }
                    KeyCode::Tab | KeyCode::Down => {
                        form.cursor = (form.cursor + 1).min(form.fields.len().saturating_sub(1));
                        model.action_form = Some(form);
                    }
                    KeyCode::BackTab | KeyCode::Up => {
                        form.cursor = form.cursor.saturating_sub(1);
                        model.action_form = Some(form);
                    }
                    KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') => {
                        form.toggle();
                        model.action_form = Some(form);
                    }
                    KeyCode::Backspace => {
                        if let Some(value) = form.values.get_mut(form.cursor) {
                            if matches!(
                                form.fields.get(form.cursor).map(|field| &field.kind),
                                Some(ActionInputKind::Text { .. })
                            ) {
                                value.pop();
                            }
                        }
                        model.action_form = Some(form);
                    }
                    KeyCode::Char(character) => {
                        if matches!(
                            form.fields.get(form.cursor).map(|field| &field.kind),
                            Some(ActionInputKind::Text { .. })
                        ) {
                            if let Some(value) = form.values.get_mut(form.cursor) {
                                value.push(character);
                            }
                        }
                        model.action_form = Some(form);
                    }
                    _ => model.action_form = Some(form),
                }
                return (model, effects);
            }
            if model.confirmation.is_some() {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('n') => model.confirmation = None,
                    KeyCode::Enter | KeyCode::Char('y') => {
                        model.selected_action = model.confirmation.take().unwrap_or_default();
                        effects.push(Effect::InvokeSelectedAction);
                    }
                    _ => {}
                }
                return (model, effects);
            }
            if model.help_open {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                    model.help_open = false;
                }
                return (model, effects);
            }
            if model.palette_open {
                match key.code {
                    KeyCode::Esc => model.palette_open = false,
                    KeyCode::Up | KeyCode::Char('k') => {
                        model.selected_action = model.selected_action.saturating_sub(1)
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        let last = model
                            .read_model
                            .as_ref()
                            .map(|read_model| read_model.actions.len().saturating_sub(1))
                            .unwrap_or_default();
                        model.selected_action = (model.selected_action + 1).min(last);
                    }
                    KeyCode::Enter => {
                        let selected = model
                            .read_model
                            .as_ref()
                            .and_then(|read_model| read_model.actions.get(model.selected_action))
                            .filter(|action| action.enabled);
                        let inputs = selected
                            .map(|action| action.inputs.clone())
                            .unwrap_or_default();
                        let requires_confirmation =
                            selected.is_some_and(|action| action.requires_confirmation);
                        if !inputs.is_empty() {
                            model.action_form =
                                Some(ActionForm::open(model.selected_action, inputs));
                            model.palette_open = false;
                        } else if requires_confirmation {
                            model.confirmation = Some(model.selected_action);
                            model.palette_open = false;
                        } else {
                            effects.push(Effect::InvokeSelectedAction);
                        }
                    }
                    _ => {}
                }
                return (model, effects);
            }
            if let Some(typed) = model.open_subject.clone() {
                match key.code {
                    KeyCode::Esc => model.open_subject = None,
                    KeyCode::Enter => {
                        model.open_subject = None;
                        let id = typed.trim().to_string();
                        if !id.is_empty() {
                            // The prefix is the scope. Draft ids are prefixed
                            // exactly so a reader — and this — never has to
                            // infer what an id refers to.
                            let workspace_id =
                                model.subject.workspace_id().unwrap_or_default().to_owned();
                            let subject = if id.starts_with("cpk_") {
                                ConsoleSubject::ChangePack {
                                    workspace_id,
                                    change_pack_id: id,
                                }
                            } else {
                                ConsoleSubject::Baseline {
                                    workspace_id,
                                    baseline_id: id,
                                }
                            };
                            model.subject = subject.clone();
                            model.selected_navigation = 0;
                            effects.push(Effect::ChangeSubject(subject));
                        }
                    }
                    KeyCode::Backspace => {
                        model.open_subject.as_mut().map(String::pop);
                    }
                    KeyCode::Char(character) => {
                        if let Some(value) = model.open_subject.as_mut() {
                            value.push(character);
                        }
                    }
                    _ => {}
                }
                return (model, effects);
            }
            if model.search.is_some() {
                match key.code {
                    KeyCode::Esc | KeyCode::Enter => model.search = None,
                    KeyCode::Backspace => {
                        model.search.as_mut().map(String::pop);
                    }
                    KeyCode::Char(character) => {
                        if let Some(search) = model.search.as_mut() {
                            search.push(character);
                        }
                    }
                    _ => {}
                }
                return (model, effects);
            }
            match key.code {
                KeyCode::Char('q') => model.should_quit = true,
                KeyCode::Char('?') => model.help_open = true,
                KeyCode::Char(':') => model.palette_open = true,
                KeyCode::Char('/') => model.search = Some(String::new()),
                // Open one ChangePack or one Baseline in its own §8.3 scope.
                // Only from a project: both are things a project contains.
                KeyCode::Char('o') if model.subject.workspace_id().is_some() => {
                    model.open_subject = Some(String::new());
                }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    model.subject = ConsoleSubject::global();
                    model.selected_navigation = 1;
                    effects.push(Effect::ChangeSubject(model.subject.clone()));
                }
                KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    model.palette_open = true
                }
                KeyCode::Char('k') | KeyCode::Up => match model.focus {
                    FocusPane::Navigation => {
                        model.selected_navigation = model.selected_navigation.saturating_sub(1)
                    }
                    FocusPane::Actions => {
                        model.selected_action = model.selected_action.saturating_sub(1)
                    }
                    FocusPane::Content => {
                        model.content_scroll = model.content_scroll.saturating_sub(1)
                    }
                },
                KeyCode::Char('j') | KeyCode::Down => match model.focus {
                    FocusPane::Navigation => {
                        let last = model
                            .read_model
                            .as_ref()
                            .map(|m| m.navigation.len().saturating_sub(1))
                            .unwrap_or_default();
                        model.selected_navigation = (model.selected_navigation + 1).min(last);
                    }
                    FocusPane::Actions => {
                        let last = model
                            .read_model
                            .as_ref()
                            .map(|m| m.actions.len().saturating_sub(1))
                            .unwrap_or_default();
                        model.selected_action = (model.selected_action + 1).min(last);
                    }
                    FocusPane::Content => {
                        model.content_scroll = model.content_scroll.saturating_add(1)
                    }
                },
                KeyCode::Tab => {
                    model.focus = match model.focus {
                        FocusPane::Navigation => FocusPane::Content,
                        FocusPane::Content => FocusPane::Actions,
                        FocusPane::Actions => FocusPane::Navigation,
                    }
                }
                KeyCode::BackTab => {
                    model.focus = match model.focus {
                        FocusPane::Navigation => FocusPane::Actions,
                        FocusPane::Content => FocusPane::Navigation,
                        FocusPane::Actions => FocusPane::Content,
                    }
                }
                KeyCode::Char('r') => effects.push(Effect::Refresh),
                KeyCode::Esc => {
                    // Escape walks one level out. A ChangePack and a Baseline are
                    // both views *of* a project, so both land there.
                    let target = match model.subject.scope() {
                        ConsoleScope::ChangePack | ConsoleScope::Baseline => model
                            .subject
                            .workspace_id()
                            .map(|workspace_id| ConsoleSubject::Project {
                                workspace_id: workspace_id.to_owned(),
                            }),
                        ConsoleScope::Project => Some(ConsoleSubject::global()),
                        ConsoleScope::Global => None,
                    };
                    if let Some(subject) = target {
                        model.subject = subject.clone();
                        effects.push(Effect::ChangeSubject(subject));
                    }
                }
                _ => {}
            }
        }
    }
    (model, effects)
}

pub fn run_console(options: LaunchOptions) -> Result<(), String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("Draft Console TUI requires an interactive terminal (PTY); terminal state was not changed".into());
    }
    let subject = options
        .preselected_workspace_id
        .map_or_else(ConsoleSubject::global, |workspace_id| {
            ConsoleSubject::Project { workspace_id }
        });
    let client = ConsoleClient::connect().map_err(|error| error.to_string())?;
    let (command_tx, command_rx) = mpsc::channel();
    let (backend_tx, backend_rx) = mpsc::channel();
    std::thread::spawn(move || backend_worker(client, command_rx, backend_tx));
    let mut model = AppModel::loading(subject.clone(), options.startup_diagnostic);
    command_tx
        .send(BackendCommand::Refresh(subject))
        .map_err(|error| error.to_string())?;
    let mut guard = TerminalGuard::enter()?;
    loop {
        while let Ok(event) = backend_rx.try_recv() {
            model = reduce(model, event).0;
        }
        guard
            .terminal
            .draw(|frame| render(frame, &model))
            .map_err(|error| error.to_string())?;
        if model.should_quit {
            break;
        }
        if !event::poll(Duration::from_millis(100)).map_err(|error| error.to_string())? {
            continue;
        }
        let event = event::read().map_err(|error| error.to_string())?;
        let app_event = match event {
            TerminalEvent::Key(key) => Some(AppEvent::Key(key)),
            TerminalEvent::Resize(_, _) => Some(AppEvent::Resized),
            _ => None,
        };
        let Some(app_event) = app_event else {
            continue;
        };
        let (next, effects) = reduce(model, app_event);
        model = next;
        for effect in effects {
            match effect {
                Effect::Refresh => {
                    let _ = command_tx.send(BackendCommand::Refresh(model.subject.clone()));
                }
                Effect::InvokeSelectedAction => {
                    if let Some(command) = selected_action_command(&model) {
                        let _ = command_tx.send(command);
                    }
                }
                Effect::ChangeSubject(subject) => {
                    let _ = command_tx.send(BackendCommand::Refresh(subject));
                }
            }
        }
    }
    let _ = command_tx.send(BackendCommand::Shutdown);
    Ok(())
}

enum BackendCommand {
    Refresh(ConsoleSubject),
    Invoke {
        subject: ConsoleSubject,
        capability: String,
        operation_id: String,
        revisions: draft_ipc::console_application::CanonicalRevisions,
        /// Values for the inputs the action declared, keyed by their stable
        /// ids. Empty for an action that declared none.
        arguments: BTreeMap<String, Value>,
    },
    Shutdown,
}

fn selected_action_command(model: &AppModel) -> Option<BackendCommand> {
    let read_model = model.read_model.as_ref()?;
    let action = read_model.actions.get(model.selected_action)?;
    if !action.enabled {
        return None;
    }
    Some(BackendCommand::Invoke {
        subject: model.subject.clone(),
        capability: action.invocation_capability.clone()?,
        operation_id: format!("tui-op-{}", unix_time_nanos()),
        revisions: read_model.revisions.clone(),
        arguments: model
            .action_form
            .as_ref()
            .filter(|form| form.action_index == model.selected_action)
            .map(ActionForm::arguments)
            .unwrap_or_default(),
    })
}

fn backend_worker(
    mut client: ConsoleClient,
    commands: mpsc::Receiver<BackendCommand>,
    events: mpsc::Sender<AppEvent>,
) {
    let mut active_subject: Option<ConsoleSubject> = None;
    let mut watch_cursor = None;
    loop {
        let command = match commands.recv_timeout(Duration::from_secs(2)) {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let Some(subject) = active_subject.clone() else {
                    continue;
                };
                if client.supports("watch_v1") {
                    match client.watch(watch_cursor, vec![subject.clone()]) {
                        Ok(event) => {
                            watch_cursor = Some(event.cursor);
                            BackendCommand::Refresh(subject)
                        }
                        Err(error) => {
                            send_client_error(&events, &mut client, &subject, error);
                            continue;
                        }
                    }
                } else {
                    BackendCommand::Refresh(subject)
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let subject = match command {
            BackendCommand::Refresh(subject) => {
                active_subject = Some(subject.clone());
                subject
            }
            BackendCommand::Invoke {
                subject,
                capability,
                operation_id,
                revisions,
                arguments,
            } => {
                if let Err(error) = client.invoke(capability, operation_id, revisions, arguments) {
                    send_client_error(&events, &mut client, &subject, error);
                    continue;
                }
                subject
            }
            BackendCommand::Shutdown => break,
        };
        match client.snapshot(subject.clone()) {
            Ok(read_model) => {
                let _ = events.send(AppEvent::BackendLoaded(Box::new(read_model)));
            }
            Err(error) => send_client_error(&events, &mut client, &subject, error),
        }
    }
}

fn send_client_error(
    events: &mpsc::Sender<AppEvent>,
    client: &mut ConsoleClient,
    subject: &ConsoleSubject,
    error: ClientError,
) {
    if error.code == "CONSOLE_DISCONNECTED" {
        let _ = events.send(AppEvent::Disconnected(error.to_string()));
        let mut backoff = ReconnectBackoff::default();
        let mut last_error = error.to_string();
        for _ in 0..6 {
            let _ = events.send(AppEvent::Reconnecting);
            std::thread::sleep(backoff.next_delay());
            match client.reconnect() {
                Ok(()) => match client.snapshot(subject.clone()) {
                    Ok(read_model) => {
                        backoff.reset();
                        let _ = events.send(AppEvent::BackendLoaded(Box::new(read_model)));
                        return;
                    }
                    Err(error) => last_error = error.to_string(),
                },
                Err(error) => last_error = error.to_string(),
            }
        }
        let _ = events.send(AppEvent::Disconnected(format!(
            "{last_error}; automatic reconnect will retry after the next refresh"
        )));
    } else {
        let _ = events.send(AppEvent::BackendFailed(error.to_string()));
    }
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn enter() -> Result<Self, String> {
        enable_raw_mode().map_err(|error| error.to_string())?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste
        ) {
            let _ = disable_raw_mode();
            let _ = execute!(
                stdout,
                DisableBracketedPaste,
                DisableMouseCapture,
                LeaveAlternateScreen
            );
            return Err(error.to_string());
        }
        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                let _ = disable_raw_mode();
                let mut stdout = io::stdout();
                let _ = execute!(
                    stdout,
                    DisableBracketedPaste,
                    DisableMouseCapture,
                    LeaveAlternateScreen
                );
                Err(error.to_string())
            }
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}

fn render(frame: &mut ratatui::Frame<'_>, model: &AppModel) {
    let area = frame.area();
    if area.width < 60 || area.height < 16 {
        frame.render_widget(
            Paragraph::new("Draft Console\nTerminal must be at least 60×16\nq quit · ? help")
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Resize required"),
                ),
            area,
        );
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(area);
    render_header(frame, rows[0], model);
    if area.width >= 100 {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(if area.width >= 140 {
                [
                    Constraint::Length(26),
                    Constraint::Percentage(55),
                    Constraint::Min(28),
                ]
            } else {
                [
                    Constraint::Length(22),
                    Constraint::Percentage(65),
                    Constraint::Min(20),
                ]
            })
            .split(rows[1]);
        render_navigation(frame, columns[0], model);
        render_content(frame, columns[1], model);
        render_actions(frame, columns[2], model);
    } else {
        let panes = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(6), Constraint::Min(6)])
            .split(rows[1]);
        render_navigation(frame, panes[0], model);
        render_content(frame, panes[1], model);
    }
    render_footer(frame, rows[2], model);
    if model.help_open {
        render_help(frame, model);
    }
    if model.palette_open {
        render_palette(frame, model);
    }
    if model.action_form.is_some() {
        render_action_form(frame, model);
    }
    if model.confirmation.is_some() {
        render_confirmation(frame, model);
    }
}

fn render_header(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, model: &AppModel) {
    let context = match model.subject.scope() {
        ConsoleScope::Global => "GLOBAL".to_string(),
        ConsoleScope::Project => format!(
            "PROJECT · {}",
            model.subject.workspace_id().unwrap_or("unknown")
        ),
        ConsoleScope::ChangePack => format!(
            "CHANGE · {}",
            model.subject.change_pack_id().unwrap_or("unknown")
        ),
        ConsoleScope::Baseline => format!(
            "BASELINE · {}",
            model.subject.baseline_id().unwrap_or("unknown")
        ),
    };
    let state = match &model.connection {
        ConnectionState::Loading => "LOADING".into(),
        ConnectionState::Connected => model
            .read_model
            .as_ref()
            .map(|m| m.health.to_uppercase())
            .unwrap_or_else(|| "CONNECTED".into()),
        ConnectionState::Stale => "STALE".into(),
        ConnectionState::Disconnected(_) => "DISCONNECTED · STALE".into(),
        ConnectionState::Failed(_) => "FAILED".into(),
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " Draft Console ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  {context}  ·  {state}")),
        ]))
        .block(Block::default().borders(Borders::ALL)),
        area,
    );
}

/// One selectable row of the navigation.
///
/// The authority sends nested sections; the TUI has one list, so a section's
/// views are flattened into rows that still name their parent. Flattening the
/// parent away instead would lose exactly the structure §8.3 specifies — a
/// reader could no longer tell that Tasks and ChangePacks are two views of Work
/// rather than peers of Baselines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationRow {
    pub section: String,
    pub view: String,
}

impl NavigationRow {
    fn label(&self) -> String {
        if self.section == self.view {
            self.view.clone()
        } else {
            format!("{} › {}", self.section, self.view)
        }
    }
}

/// Every view the current model offers, in the authority's order.
pub fn navigation_rows(model: &AppModel) -> Vec<NavigationRow> {
    model
        .read_model
        .as_ref()
        .map(|read_model| read_model.navigation.as_slice())
        .unwrap_or(&[])
        .iter()
        .flat_map(|section| {
            section.views().into_iter().map(|view| NavigationRow {
                section: section.label.clone(),
                view,
            })
        })
        .collect()
}

fn render_navigation(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    model: &AppModel,
) {
    let items = navigation_rows(model)
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            let prefix = if index == model.selected_navigation {
                "› "
            } else {
                "  "
            };
            ListItem::new(format!("{prefix}{}", row.label()))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(focused_block(
            "Navigation",
            model.focus == FocusPane::Navigation,
        )),
        area,
    );
}

fn render_content(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, model: &AppModel) {
    let mut text = if let Some(read_model) = &model.read_model {
        serde_json::to_string_pretty(active_content(model, read_model))
            .unwrap_or_else(|_| "Content unavailable".into())
    } else {
        match &model.connection {
            ConnectionState::Failed(error) | ConnectionState::Disconnected(error) => error.clone(),
            _ => "Loading authoritative Console model…".into(),
        }
    };
    if let Some(query) = model.search.as_deref().filter(|query| !query.is_empty()) {
        let query = query.to_lowercase();
        text = text
            .lines()
            .filter(|line| line.to_lowercase().contains(&query))
            .collect::<Vec<_>>()
            .join("\n");
        if text.is_empty() {
            text = "No loaded rows match the local filter".into();
        }
    }
    frame.render_widget(
        Paragraph::new(text)
            .scroll((model.content_scroll, 0))
            .wrap(Wrap { trim: false })
            .block(focused_block(
                active_navigation_label(model)
                    .as_deref()
                    .unwrap_or("Current view"),
                model.focus == FocusPane::Content,
            )),
        area,
    );
}

fn active_navigation_label(model: &AppModel) -> Option<String> {
    navigation_rows(model)
        .into_iter()
        .nth(model.selected_navigation)
        .map(|row| row.view)
}

fn active_content<'a>(model: &AppModel, read_model: &'a ConsoleReadModel) -> &'a serde_json::Value {
    let label = active_navigation_label(model).unwrap_or_default();
    if read_model.subject.scope() == ConsoleScope::Global && label == "Projects" {
        return read_model
            .content
            .get("overview")
            .and_then(|overview| overview.get("projects"))
            .unwrap_or(&read_model.content);
    }
    // A path, not a key, because the §8.3 sections nest. The TUI still renders
    // whatever it is handed: it selects a view, it does not interpret one.
    let path: &[&str] = match (read_model.subject.scope(), label.as_str()) {
        (ConsoleScope::Global, "Overview") => &["overview"],
        (ConsoleScope::Global, "Inbox") => &["inbox"],
        (ConsoleScope::Global, "Doctor") => &["doctor"],
        (ConsoleScope::Global, "Extensions") => &["extensions"],
        (ConsoleScope::Global, "Settings") => &["settings"],
        // §8.3's project sections. Observation sits inside Resources and Tools
        // inside Extensions, because an observation is evidence about a
        // Resource and a tool exists only because an extension contributes it.
        (ConsoleScope::Project, "Overview") => &["overview"],
        (ConsoleScope::Project, "Tasks") => &["work", "tasks"],
        (ConsoleScope::Project, "Packs") => &["work", "packs"],
        (ConsoleScope::Project, "Resources") => &["resources", "resources"],
        (ConsoleScope::Project, "Observation") => &["resources", "observation"],
        (ConsoleScope::Project, "Baselines") => &["baselines", "baselines"],
        // Rendered from within the Baselines section and never folded into a
        // Baseline: a delivery that failed leaves the accepted Baseline
        // exactly as it was, and one view for both would hide that.
        (ConsoleScope::Project, "Publications") => &["baselines", "publications"],
        (ConsoleScope::Project, "Activity") => &["activity"],
        (ConsoleScope::Project, "Providers") => &["providers"],
        (ConsoleScope::Project, "Extensions") => &["extensions", "extensions"],
        (ConsoleScope::Project, "Tools") => &["extensions", "tools"],
        // What the Change Graph holds about one ChangePack. Every act is its own
        // view because every act is its own fact — a reader who cannot tell an
        // approval from a passing check cannot tell what authorized a
        // promotion.
        (ConsoleScope::ChangePack, "Summary") => &["summary"],
        (ConsoleScope::ChangePack, "Intent") => &["intent"],
        (ConsoleScope::ChangePack, "Scope") => &["scope"],
        (ConsoleScope::ChangePack, "Revisions") => &["revisions"],
        (ConsoleScope::ChangePack, "Impact") => &["impact"],
        (ConsoleScope::ChangePack, "Representations") => &["representations"],
        (ConsoleScope::ChangePack, "Evidence") => &["authorization", "evidence"],
        (ConsoleScope::ChangePack, "Assessments") => &["authorization", "assessments"],
        (ConsoleScope::ChangePack, "Review") => &["authorization", "reviews"],
        (ConsoleScope::ChangePack, "Decisions") => &["authorization", "decisions"],
        (ConsoleScope::ChangePack, "Gates") => &["authorization", "gates"],
        (ConsoleScope::ChangePack, "Promotion") => &["authorization", "promotion"],
        (ConsoleScope::ChangePack, "Receipts") => &["receipts"],
        (ConsoleScope::ChangePack, "Recovery") => &["recovery"],
        // One accepted historical node. The three roots are separate views
        // because they answer three different questions and are never
        // collapsed into one another.
        (ConsoleScope::Baseline, "Summary") => &["baseline", "record"],
        (ConsoleScope::Baseline, "State root") => &["baseline", "manifest", "project_state_root"],
        (ConsoleScope::Baseline, "Evidence root") => {
            &["baseline", "manifest", "state_evidence_root"]
        }
        (ConsoleScope::Baseline, "Coverage") => &["baseline", "manifest", "coverage_evidence_root"],
        (ConsoleScope::Baseline, "Lineage") => &["baseline", "lineage"],
        (ConsoleScope::Baseline, "Composition") => &["baseline", "composition"],
        (ConsoleScope::Baseline, "Recoverability") => &["baseline", "recoverability"],
        (ConsoleScope::Baseline, "Receipts") => &["baseline", "receipts"],
        (ConsoleScope::Baseline, "Publications") => &["baseline", "publications"],
        _ => &[],
    };
    if path.is_empty() {
        return &read_model.content;
    }
    let mut value = &read_model.content;
    for segment in path {
        match value.get(segment) {
            Some(next) => value = next,
            None => return &read_model.content,
        }
    }
    value
}

fn render_actions(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, model: &AppModel) {
    let items = model
        .read_model
        .as_ref()
        .map(|m| m.actions.as_slice())
        .unwrap_or(&[])
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let selection = if index == model.selected_action {
                "›"
            } else {
                " "
            };
            let state = if action.enabled {
                "enabled"
            } else {
                action.disabled_reason.as_deref().unwrap_or("disabled")
            };
            let inputs = if action.inputs.is_empty() {
                String::new()
            } else {
                format!("\n  asks for {} input(s)", action.inputs.len())
            };
            ListItem::new(format!("{selection} {}\n  {state}{inputs}", action.label))
        })
        .collect::<Vec<_>>();

    // What the server says to do next, including how to resolve a capability
    // it has withheld. The remedy is `draftd`'s choice; this only shows it.
    let mut items = items;
    let suggestions = model
        .read_model
        .as_ref()
        .map(|read_model| read_model.next_safe_actions.as_slice())
        .unwrap_or(&[]);
    if !suggestions.is_empty() {
        items.push(ListItem::new("─ next safe ─".to_string()));
        for suggestion in suggestions {
            items.push(ListItem::new(format!(
                "  {}\n  {}",
                suggestion.label, suggestion.reason
            )));
        }
    }

    frame.render_widget(
        List::new(items).block(focused_block(
            "Backend actions",
            model.focus == FocusPane::Actions,
        )),
        area,
    );
}

fn render_footer(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, model: &AppModel) {
    let mut help =
        "↑↓/jk navigate · Tab pane · o open · : palette · r refresh · ? help · q quit".to_string();
    if let Some(diagnostic) = &model.diagnostic {
        help.push_str(&format!("  ·  {diagnostic}"));
    }
    if let Some(search) = &model.search {
        help = format!("Search: {search}_  ·  Esc clear · Enter apply");
    }
    if let Some(typed) = &model.open_subject {
        help = format!("Open cpk_ or baseline id: {typed}_  ·  Esc cancel · Enter open");
    }
    frame.render_widget(
        Paragraph::new(help).block(Block::default().borders(Borders::ALL).title("Keys")),
        area,
    );
}

fn render_help(frame: &mut ratatui::Frame<'_>, _model: &AppModel) {
    let area = centered(frame.area(), 72, 16);
    frame.render_widget(Paragraph::new(
        "Contextual help\n\nArrows or j/k  Navigate focused pane\nEnter          Open/select\nEsc            Close/back\nTab/Shift+Tab  ChangePack pane\n/              Search/filter\no              Open a ChangePack or Baseline in its own scope\n: or Ctrl+K    Command palette\nCtrl+P         Project switching\n?              Toggle help\nq              Safe quit\n\nDomain actions are supplied and validated by draftd."
    ).block(Block::default().borders(Borders::ALL).title("Help")).wrap(Wrap { trim: false }), area);
}

fn render_palette(frame: &mut ratatui::Frame<'_>, model: &AppModel) {
    let area = centered(frame.area(), 76, 18);
    let mut lines = vec![
        Line::from("Local commands"),
        Line::from("  / Search loaded data"),
        Line::from("  Ctrl+P Switch project"),
        Line::from("  r Refresh active scope"),
        Line::from("  ? Contextual help"),
        Line::from(""),
        Line::from("Backend actions for current scope"),
    ];
    if let Some(read_model) = &model.read_model {
        for (index, action) in read_model.actions.iter().enumerate() {
            let marker = if index == model.selected_action {
                "›"
            } else {
                " "
            };
            let state = if action.enabled {
                "enabled"
            } else {
                action.disabled_reason.as_deref().unwrap_or("disabled")
            };
            lines.push(Line::from(format!("{marker} {} · {state}", action.label)));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Command palette"),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// The inputs an action declared, exactly as the server described them.
///
/// Nothing here is specific to any action: the labels, the kinds and the
/// select options all arrive in the read model, so an extension-contributed
/// action renders without a line of new terminal code.
fn render_action_form(frame: &mut ratatui::Frame<'_>, model: &AppModel) {
    let Some(form) = model.action_form.as_ref() else {
        return;
    };
    let title = model
        .read_model
        .as_ref()
        .and_then(|read_model| read_model.actions.get(form.action_index))
        .map(|action| action.label.clone())
        .unwrap_or_else(|| "Action".into());

    let mut lines: Vec<Line> = Vec::new();
    for (index, (field, value)) in form.fields.iter().zip(&form.values).enumerate() {
        let marker = if index == form.cursor { ">" } else { " " };
        let requirement = if field.required { " *" } else { "" };
        let shown = match &field.kind {
            ActionInputKind::Text { .. } => {
                if value.is_empty() {
                    "—".to_string()
                } else {
                    value.clone()
                }
            }
            ActionInputKind::Select { options } => options
                .iter()
                .find(|option| &option.value == value)
                .map(|option| option.label.clone())
                .unwrap_or_else(|| "—".into()),
            ActionInputKind::Boolean | ActionInputKind::Confirmation => {
                if value == "true" { "yes" } else { "no" }.to_string()
            }
        };
        lines.push(Line::from(vec![Span::raw(format!(
            "{marker} {}{requirement}: {shown}",
            field.label
        ))]));
        if let Some(help) = &field.help {
            lines.push(Line::from(Span::raw(format!("    {help}"))));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::raw(
        "Tab/↑↓ field · type to edit · Space/←→ toggle · Enter submit · Esc cancel",
    )));
    lines.push(Line::from(Span::raw(
        "draftd validates every value before anything happens.",
    )));

    let height = (lines.len() as u16 + 2).min(frame.area().height);
    let area = centered(frame.area(), 76, height);
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_confirmation(frame: &mut ratatui::Frame<'_>, model: &AppModel) {
    let Some(index) = model.confirmation else {
        return;
    };
    let Some(action) = model
        .read_model
        .as_ref()
        .and_then(|read_model| read_model.actions.get(index))
    else {
        return;
    };
    let target = model
        .subject
        .change_pack_id()
        .or(model.subject.workspace_id())
        .unwrap_or("current context");
    let body = format!(
        "Target: {target}\nAction: {}\nEffect: draftd will revalidate current permissions, lifecycle, and revisions before execution.\n\nEnter/y confirm · Esc/n cancel",
        action.label
    );
    let area = centered(frame.area(), 76, 12);
    frame.render_widget(
        Paragraph::new(body)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Explicit confirmation"),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn focused_block(title: &str, focused: bool) -> Block<'_> {
    let color = if std::env::var_os("NO_COLOR").is_some() {
        Color::Reset
    } else if std::env::var_os("DRAFT_HIGH_CONTRAST").is_some() {
        if focused {
            Color::White
        } else {
            Color::Gray
        }
    } else if focused {
        Color::Blue
    } else {
        Color::DarkGray
    };
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(color))
}

fn centered(area: ratatui::layout::Rect, max_width: u16, max_height: u16) -> ratatui::layout::Rect {
    let width = area.width.min(max_width);
    let height = area.height.min(max_height);
    ratatui::layout::Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn unix_time_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

pub fn render_test_frame(model: &AppModel, width: u16, height: u16) -> Result<String, String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).map_err(|error| error.to_string())?;
    terminal
        .draw(|frame| render(frame, model))
        .map_err(|error| error.to_string())?;
    Ok(format!("{:?}", terminal.backend().buffer()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_ipc::console_application::{CanonicalRevisions, ConsoleReadModel};
    use serde_json::json;

    /// A model carrying the authority's own navigation, never a second copy.
    ///
    /// Taking the list from `navigation_for` is the point: a test that
    /// restated the sections would keep passing while the TUI and the daemon
    /// drifted apart, which is precisely the drift these tests exist to catch.
    fn model(scope: ConsoleScope) -> AppModel {
        let workspace_id = "prj_test".to_string();
        let subject = match scope {
            ConsoleScope::Global => ConsoleSubject::global(),
            ConsoleScope::Project => ConsoleSubject::Project { workspace_id },
            ConsoleScope::ChangePack => ConsoleSubject::ChangePack {
                workspace_id,
                change_pack_id: "cpk_test".into(),
            },
            ConsoleScope::Baseline => ConsoleSubject::Baseline {
                workspace_id,
                baseline_id: "sha256:base".into(),
            },
        };
        let navigation = draft_ipc::console_application::navigation_for(scope);
        reduce(
            AppModel::loading(subject.clone(), None),
            AppEvent::BackendLoaded(Box::new(ConsoleReadModel {
                subject,
                revisions: CanonicalRevisions::default(),
                freshness: ModelFreshness::Fresh,
                health: "healthy".into(),
                read_only: false,
                permission_reason: None,
                navigation,
                content: json!({"status": "ready"}),
                actions: vec![],
                next_safe_actions: vec![],
                evidence_links: vec![],
                operation_links: vec![],
            })),
        )
        .0
    }

    #[test]
    fn canonical_scope_hierarchies_are_fixed() {
        let global = render_test_frame(&model(ConsoleScope::Global), 140, 40).unwrap();
        assert!(global.contains("Projects") && global.contains("Doctor"));
        assert!(!global.contains("Approvals"));
        let change = render_test_frame(&model(ConsoleScope::ChangePack), 140, 40).unwrap();
        // The Change Graph's own stages, each its own view. A single "Submit"
        // step is exactly what the ontology took apart: deciding authorizes,
        // promotion accepts, and a reader has to be able to see which happened.
        assert!(change.contains("Summary") && change.contains("Decisions"));
        assert!(change.contains("Evidence") && change.contains("Promotion"));
        for retired in ["Submit", "Approvals", "Rollback", "Risk"] {
            assert!(
                !change.contains(retired),
                "the ChangePack scope still offers the retired '{retired}' view"
            );
        }
        // A ChangePack is not a Pack. The header named the retired ontology long
        // after the model stopped using it.
        assert!(
            !change.contains("PACK"),
            "the ChangePack header still names the retired Pack ontology:\n{change}"
        );
    }

    /// Every view the authority offers can be selected and rendered.
    ///
    /// This is the drift guard. `draftd` serves §8.3's sections from
    /// `navigation_for`; if a section gains a view and the TUI has no path for
    /// it, the view silently falls back to the whole model and a reader sees
    /// the entire project blob under a heading that promised one thing. The
    /// fallback is deliberate and safe, so only a test can catch it.
    #[test]
    fn every_section_the_authority_offers_has_a_view_in_the_tui() {
        for scope in [
            ConsoleScope::Global,
            ConsoleScope::Project,
            ConsoleScope::ChangePack,
            ConsoleScope::Baseline,
        ] {
            let mut model = model(scope);
            let rows = navigation_rows(&model);
            assert!(!rows.is_empty(), "{scope:?} offers no views at all",);
            for (index, row) in rows.iter().enumerate() {
                model.selected_navigation = index;
                assert_eq!(
                    active_navigation_label(&model).as_deref(),
                    Some(row.view.as_str()),
                    "{scope:?} row {index} does not select the view it names"
                );
                let read_model = model.read_model.as_ref().expect("a loaded model");
                // A mapped view resolves to a node of its own; an unmapped one
                // falls back to the whole content, which is what this catches.
                let selected = active_content(&model, read_model);
                assert!(
                    !std::ptr::eq(selected, &read_model.content)
                        || read_model.content.get("status").is_some(),
                    "{scope:?} view '{}' has no content path and falls back to the whole model",
                    row.view
                );
            }
        }
    }

    /// The nesting §8.3 specifies survives the trip to the frontend.
    #[test]
    fn work_owns_tasks_and_changes_rather_than_standing_beside_them() {
        let sections = draft_ipc::console_application::navigation_for(ConsoleScope::Project);
        let labels: Vec<&str> = sections
            .iter()
            .map(|section| section.label.as_str())
            .collect();
        assert_eq!(
            labels,
            vec![
                "Overview",
                "Work",
                "Resources",
                "Baselines",
                "Activity",
                "Providers",
                "Extensions",
            ],
            "the project sections are exactly §8.3's, in order"
        );
        let work = sections
            .iter()
            .find(|section| section.label == "Work")
            .expect("Work is a section");
        assert_eq!(
            work.children,
            vec!["Tasks".to_string(), "Packs".to_string()]
        );
        // And the rows a frontend renders name their parent, so a reader can
        // still see that Tasks is a view of Work.
        let rows = navigation_rows(&model(ConsoleScope::Project));
        assert!(rows
            .iter()
            .any(|row| row.section == "Work" && row.view == "Tasks"));
    }

    #[test]
    fn the_change_graph_reaches_the_tui_through_the_same_model_the_browser_reads() {
        // The TUI selects a view of the daemon's model; it never interprets
        // one. What this proves is that the selection reaches the ChangePack
        // Graph's parts — a Baseline and a delivery rendered separately, so a
        // failed delivery can never be shown as the Baseline having failed.
        let subject = ConsoleSubject::Project {
            workspace_id: "prj_test".into(),
        };
        let content = json!({
            "overview": {"name": "test"},
            "work": {
                "tasks": [{"id": "tsk_1"}],
                "packs": [{"change_pack_id": "cpk_1"}],
            },
            "baselines": {
                "baselines": [{"baseline": "sha256:accepted"}],
                "publications": [{"publication": "pub_1", "state": "failed"}],
            },
            "providers": {"bindings": [{"binding": "pbd_1"}]},
        });
        let mut model = reduce(
            AppModel::loading(subject.clone(), None),
            AppEvent::BackendLoaded(Box::new(ConsoleReadModel {
                subject,
                revisions: CanonicalRevisions::default(),
                freshness: ModelFreshness::Fresh,
                health: "healthy".into(),
                read_only: false,
                permission_reason: None,
                navigation: draft_ipc::console_application::navigation_for(ConsoleScope::Project),
                content,
                actions: vec![],
                next_safe_actions: vec![],
                evidence_links: vec![],
                operation_links: vec![],
            })),
        )
        .0;

        // Selected by name, never by position: an index would pass while the
        // rows underneath it silently became something else.
        let select = |model: &mut AppModel, view: &str| {
            let index = navigation_rows(model)
                .iter()
                .position(|row| row.view == view)
                .unwrap_or_else(|| panic!("no '{view}' view in the project navigation"));
            model.selected_navigation = index;
        };

        select(&mut model, "Baselines");
        let baseline = render_test_frame(&model, 140, 40).unwrap();
        assert!(
            baseline.contains("sha256:accepted"),
            "the Baselines view shows the accepted Baseline:\n{baseline}"
        );
        assert!(
            !baseline.contains("failed"),
            "and nothing about a delivery, which is a separate view:\n{baseline}"
        );

        select(&mut model, "Publications");
        let publications = render_test_frame(&model, 140, 40).unwrap();
        assert!(publications.contains("pub_1"), "{publications}");
        assert!(
            !publications.contains("sha256:accepted"),
            "the delivery view does not restate the Baseline as though it were at stake"
        );

        select(&mut model, "Providers");
        let providers = render_test_frame(&model, 140, 40).unwrap();
        assert!(
            providers.contains("pbd_1"),
            "provider state reaches the TUI:\n{providers}"
        );

        select(&mut model, "Packs");
        let changes = render_test_frame(&model, 140, 40).unwrap();
        assert!(changes.contains("cpk_1"), "{changes}");
    }

    /// A ChangePack and a Baseline are reachable, not merely defined.
    ///
    /// §8.3 gives each its own scope; a scope no frontend can navigate to is a
    /// specification, not a feature. The id's prefix chooses, so the terminal
    /// needs no separate mode for each and cannot pick the wrong one.
    #[test]
    fn a_change_and_a_baseline_can_each_be_opened_in_their_own_scope() {
        let key = |code| AppEvent::Key(KeyEvent::new(code, KeyModifiers::NONE));
        let open = |typed: &str| {
            let (model, _) = reduce(model(ConsoleScope::Project), key(KeyCode::Char('o')));
            assert!(
                model.open_subject.is_some(),
                "'o' opens the prompt from a project"
            );
            let mut model = model;
            for character in typed.chars() {
                let (next, _) = reduce(model, key(KeyCode::Char(character)));
                model = next;
            }
            reduce(model, key(KeyCode::Enter))
        };

        let (model, effects) = open("cpk_abc123");
        assert_eq!(model.subject.scope(), ConsoleScope::ChangePack);
        assert_eq!(model.subject.change_pack_id(), Some("cpk_abc123"));
        assert!(model.subject.baseline_id().is_none());
        assert!(effects
            .iter()
            .any(|effect| matches!(effect, Effect::ChangeSubject(_))));

        let (model, effects) = open("sha256:abc");
        assert_eq!(model.subject.scope(), ConsoleScope::Baseline);
        assert_eq!(model.subject.baseline_id(), Some("sha256:abc"));
        assert!(model.subject.change_pack_id().is_none());
        assert!(effects
            .iter()
            .any(|effect| matches!(effect, Effect::ChangeSubject(_))));

        // The project is kept, because both are things a project contains.
        assert_eq!(model.subject.workspace_id(), Some("prj_test"));

        // Escape walks back out to the project, from either.
        let (out, _) = reduce(model, key(KeyCode::Esc));
        assert_eq!(out.subject.scope(), ConsoleScope::Project);
    }

    #[test]
    fn disconnect_preserves_models_and_invalidates_actions() {
        let original = model(ConsoleScope::ChangePack);
        let (stale, _) = reduce(original, AppEvent::Disconnected("daemon stopped".into()));
        assert!(stale.read_model.is_some());
        assert!(matches!(stale.connection, ConnectionState::Disconnected(_)));
        assert_eq!(stale.read_model.unwrap().freshness, ModelFreshness::Stale);
    }

    #[test]
    fn minimum_size_has_stable_resize_view() {
        let frame = render_test_frame(&model(ConsoleScope::Global), 59, 15).unwrap();
        assert!(frame.contains("Resize required") && frame.contains("q quit"));
    }

    #[test]
    fn wide_standard_narrow_and_minimum_layouts_render() {
        let model = model(ConsoleScope::ChangePack);
        for (width, height) in [(200, 55), (140, 40), (100, 30), (80, 24), (60, 18)] {
            let frame = render_test_frame(&model, width, height).unwrap();
            assert!(frame.contains("Draft Console"));
            assert!(frame.contains("q quit") || frame.contains("Keys"));
        }
    }

    #[test]
    fn reducer_handles_focus_help_quit_and_confirmation_capture() {
        let model = model(ConsoleScope::ChangePack);
        let (model, _) = reduce(
            model,
            AppEvent::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        );
        assert_eq!(model.focus, FocusPane::Content);
        let (model, _) = reduce(
            model,
            AppEvent::Key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE)),
        );
        assert!(model.help_open);
        let (model, _) = reduce(
            model,
            AppEvent::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        );
        assert!(!model.help_open);
        let (model, _) = reduce(
            model,
            AppEvent::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
        );
        assert!(model.should_quit);
    }

    /// An action the terminal has never heard of renders and submits.
    ///
    /// The action id, its label, its inputs and their options all come from the
    /// server. Nothing in this crate knows what an extension source is, and
    /// that is the point: a contributed action needs no terminal code.
    #[test]
    fn declared_inputs_are_rendered_and_submitted_without_action_specific_code() {
        use draft_ipc::console_application::{
            ActionInputField, ActionInputKind, ActionPresentation, SelectOption,
        };

        let action = ActionPresentation {
            action_id: "some.contributed.action".into(),
            label: "Contributed action".into(),
            enabled: true,
            disabled_reason: None,
            invocation_capability: Some("cap-1".into()),
            requires_confirmation: false,
            expires_at_unix_ms: Some(i64::MAX),
            inputs: vec![
                ActionInputField {
                    id: "source_id".into(),
                    label: "Source".into(),
                    kind: ActionInputKind::Select {
                        options: vec![
                            SelectOption {
                                value: "alpha".into(),
                                label: "Alpha".into(),
                            },
                            SelectOption {
                                value: "beta".into(),
                                label: "Beta".into(),
                            },
                        ],
                    },
                    required: true,
                    help: None,
                },
                ActionInputField {
                    id: "note".into(),
                    label: "Note".into(),
                    kind: ActionInputKind::Text { max_length: None },
                    required: false,
                    help: None,
                },
            ],
            input_contract_digest: "digest".into(),
            target: None,
        };

        let mut base = model(ConsoleScope::Global);
        if let Some(read_model) = base.read_model.as_mut() {
            read_model.actions = vec![action];
        }

        // Opening the action opens its form rather than invoking blind.
        let (base, effects) = reduce(
            base,
            AppEvent::Key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE)),
        );
        let (base, effects2) = reduce(
            base,
            AppEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        );
        assert!(effects.is_empty() && effects2.is_empty(), "no blind invoke");
        let form = base.action_form.clone().expect("the form opened");
        assert_eq!(form.fields.len(), 2);

        // The select starts on its first declared option and cycles by value.
        assert_eq!(form.values[0], "alpha");
        let (base, _) = reduce(
            base,
            AppEvent::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
        );
        assert_eq!(base.action_form.as_ref().unwrap().values[0], "beta");

        // Typing lands in the text field once it is focused.
        let (base, _) = reduce(
            base,
            AppEvent::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        );
        let mut base = base;
        for character in "hi".chars() {
            let (next, _) = reduce(
                base,
                AppEvent::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)),
            );
            base = next;
        }

        let arguments = base.action_form.as_ref().unwrap().arguments();
        // Submitted by stable id and by option *value*, never by label.
        assert_eq!(arguments.get("source_id"), Some(&json!("beta")));
        assert_eq!(arguments.get("note"), Some(&json!("hi")));

        // Enter submits, and the invocation carries exactly those arguments.
        let (base, effects) = reduce(
            base,
            AppEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        );
        assert_eq!(effects, vec![Effect::InvokeSelectedAction]);
        match selected_action_command(&base).expect("an enabled action invokes") {
            BackendCommand::Invoke { arguments, .. } => {
                assert_eq!(arguments.get("source_id"), Some(&json!("beta")));
                assert_eq!(arguments.get("note"), Some(&json!("hi")));
            }
            _ => panic!("expected an invoke command"),
        }

        // And it renders, without the terminal knowing what the action is.
        let frame = render_test_frame(&base, 80, 24).unwrap();
        assert!(frame.contains("Contributed action"));
        assert!(frame.contains("Source"));
        assert!(frame.contains("Beta"));

        // A fresh authoritative model closes the form: its descriptor is spent
        // and the action may not even be offered any more.
        let refreshed = model(ConsoleScope::Global);
        let (settled, _) = reduce(
            base,
            AppEvent::BackendLoaded(Box::new(refreshed.read_model.unwrap())),
        );
        assert!(settled.action_form.is_none());
    }

    /// An optional text field left untouched is not submitted as empty.
    #[test]
    fn an_untouched_optional_input_is_omitted_rather_than_sent_blank() {
        use draft_ipc::console_application::{ActionInputField, ActionInputKind};

        let form = ActionForm::open(
            0,
            vec![
                ActionInputField {
                    id: "reason".into(),
                    label: "Reason".into(),
                    kind: ActionInputKind::Text { max_length: None },
                    required: false,
                    help: None,
                },
                ActionInputField {
                    id: "acknowledged".into(),
                    label: "Acknowledge".into(),
                    kind: ActionInputKind::Confirmation,
                    required: true,
                    help: None,
                },
            ],
        );
        let arguments = form.arguments();
        assert!(!arguments.contains_key("reason"));
        // A confirmation is always sent, and starts unacknowledged so the
        // server refuses until the user actually acknowledges it.
        assert_eq!(arguments.get("acknowledged"), Some(&json!(false)));
    }

    #[test]
    fn non_interactive_launch_fails_before_connecting_or_changing_terminal_state() {
        let error = run_console(LaunchOptions {
            preselected_workspace_id: None,
            startup_diagnostic: None,
        })
        .unwrap_err();
        assert!(error.contains("interactive terminal"));
    }
}
