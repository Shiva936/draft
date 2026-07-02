use std::path::Path;

use draft_core::common::WorkspacePath;
use draft_core::{App, ChangepackStatus};
use ratatui::backend::TestBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Terminal;

#[derive(Debug, Clone)]
pub struct CockpitModel {
    pub workspace_id: String,
    pub changes: Vec<String>,
    pub packs: Vec<CockpitPack>,
    pub service_state: String,
    pub blockers: Vec<String>,
    pub receipts: usize,
}

#[derive(Debug, Clone)]
pub struct CockpitPack {
    pub id: String,
    pub status: ChangepackStatus,
    pub name: String,
    pub files: Vec<WorkspacePath>,
    pub verification_count: usize,
    pub decision_count: usize,
    pub receipt_count: usize,
}

pub fn run_review_cockpit(cwd: &Path) -> Result<(), String> {
    let model = load_model(cwd)?;
    print!("{}", render_text(&model));
    Ok(())
}

pub fn load_model(cwd: &Path) -> Result<CockpitModel, String> {
    let app = App::new();
    let status = app.status(cwd).map_err(|e| e.to_string())?;
    let mut packs = Vec::new();
    for pack in app.pack_list(cwd).map_err(|e| e.to_string())? {
        let report = app
            .pack_show(cwd, pack.id.as_str())
            .map_err(|e| e.to_string())?;
        packs.push(CockpitPack {
            id: report.pack.id.to_string(),
            status: report.pack.status,
            name: report.pack.name.unwrap_or_default(),
            files: report.patch.files.iter().map(|f| f.path.clone()).collect(),
            verification_count: report.pack.verification_refs.len(),
            decision_count: report.pack.decision_refs.len(),
            receipt_count: report.pack.receipt_refs.len(),
        });
    }
    let receipts = app.receipts(cwd).map_err(|e| e.to_string())?.len();
    let mut blockers = Vec::new();
    for pack in &packs {
        if !matches!(
            pack.status,
            ChangepackStatus::Approved | ChangepackStatus::Saved
        ) {
            blockers.push(format!("{} requires approval before save", pack.id));
        }
        if pack.verification_count == 0 {
            blockers.push(format!("{} requires verification before save", pack.id));
        }
    }
    Ok(CockpitModel {
        workspace_id: status.workspace_id.to_string(),
        changes: status
            .changes
            .iter()
            .map(|c| format!("{:?} {}", c.change_kind, c.path))
            .collect(),
        packs,
        service_state: "daemon optional; direct core mode active".to_string(),
        blockers,
        receipts,
    })
}

pub fn render_text(model: &CockpitModel) -> String {
    let mut out = String::new();
    out.push_str("Draft Review Cockpit\n");
    out.push_str(&format!("Workspace: {}\n", model.workspace_id));
    out.push_str(&format!("Service: {}\n", model.service_state));
    out.push_str(&format!("Receipts: {}\n\n", model.receipts));
    out.push_str("Changepacks\n");
    if model.packs.is_empty() {
        out.push_str("  none\n");
    }
    for pack in &model.packs {
        out.push_str(&format!(
            "  {}  {:?}  {}  files={} verify={} decisions={} receipts={}\n",
            pack.id,
            pack.status,
            pack.name,
            pack.files.len(),
            pack.verification_count,
            pack.decision_count,
            pack.receipt_count
        ));
        for file in &pack.files {
            out.push_str(&format!("    file {file}\n"));
        }
    }
    out.push_str("\nCurrent Workspace Changes\n");
    if model.changes.is_empty() {
        out.push_str("  none\n");
    }
    for change in &model.changes {
        out.push_str(&format!("  {change}\n"));
    }
    out.push_str("\nPolicy And Save Readiness\n");
    if model.blockers.is_empty() {
        out.push_str("  no blockers detected\n");
    }
    for blocker in &model.blockers {
        out.push_str(&format!("  blocker: {blocker}\n"));
    }
    out.push_str("\nActions\n");
    out.push_str("  refresh verify risk approve reject compare compose save rollback quit\n");
    out
}

pub fn render_test_frame(model: &CockpitModel) -> Result<String, String> {
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;
    terminal
        .draw(|frame| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(4),
                    Constraint::Min(8),
                    Constraint::Length(8),
                ])
                .split(frame.size());
            let header = Paragraph::new(vec![
                Line::from("Draft Review Cockpit"),
                Line::from(format!("Workspace: {}", model.workspace_id)),
                Line::from(format!("Service: {}", model.service_state)),
            ])
            .block(Block::default().borders(Borders::ALL).title("Status"));
            frame.render_widget(header, chunks[0]);

            let packs = model
                .packs
                .iter()
                .map(|pack| {
                    ListItem::new(format!(
                        "{} {:?} {} files={}",
                        pack.id,
                        pack.status,
                        pack.name,
                        pack.files.len()
                    ))
                })
                .collect::<Vec<_>>();
            frame.render_widget(
                List::new(packs).block(Block::default().borders(Borders::ALL).title("Packs")),
                chunks[1],
            );

            let blockers = if model.blockers.is_empty() {
                "no blockers detected".to_string()
            } else {
                model.blockers.join("\n")
            };
            frame.render_widget(
                Paragraph::new(blockers)
                    .block(Block::default().borders(Borders::ALL).title("Readiness")),
                chunks[2],
            );
        })
        .map_err(|e| e.to_string())?;
    Ok(format!("{:?}", terminal.backend().buffer()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cockpit_text_shows_blockers_and_actions() {
        let model = CockpitModel {
            workspace_id: "ws_test".to_string(),
            changes: vec!["Modified app.txt".to_string()],
            packs: vec![CockpitPack {
                id: "pck_1".to_string(),
                status: ChangepackStatus::Draft,
                name: "demo".to_string(),
                files: vec![WorkspacePath::from("app.txt")],
                verification_count: 0,
                decision_count: 0,
                receipt_count: 0,
            }],
            service_state: "direct".to_string(),
            blockers: vec!["pck_1 requires verification before save".to_string()],
            receipts: 0,
        };
        let text = render_text(&model);
        assert!(text.contains("Draft Review Cockpit"));
        assert!(text.contains("blocker: pck_1"));
        assert!(text.contains("compare compose save rollback"));
        assert!(render_test_frame(&model)
            .unwrap()
            .contains("Draft Review Cockpit"));
    }
}
