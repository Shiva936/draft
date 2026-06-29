use std::path::PathBuf;
use draft_core::models::{ChangeGroup, RepoContext, RiskAssessment, VerificationEvidence};
use draft_core::git_adapter::GitAdapter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiMode {
    Review,
    Diff,
    Verify,
    CommitConfirm,
    Help,
    Exit,
}

pub struct TuiState {
    pub repo_root: PathBuf,
    pub repo_context: RepoContext,
    pub groups: Vec<ChangeGroup>,
    pub selected_group_index: usize,
    pub diff_scroll_y: usize,
    pub diff_lines_cache: Vec<String>,
    pub verification: Option<VerificationEvidence>,
    pub risk_summary: RiskAssessment,
    pub mode: TuiMode,
    pub commit_message: String,
    pub error_message: Option<String>,
    pub verification_logs: Vec<String>,
    pub verification_running: bool,
    pub commit_success: Option<String>,
    pub commit_input_focused: bool,
}

impl TuiState {
    pub fn new(repo_root: PathBuf, repo_context: RepoContext, groups: Vec<ChangeGroup>, verification: Option<VerificationEvidence>, commit_message: Option<String>) -> Self {
        let risk_summary = draft_core::risk_engine::RiskEngine::summarize(&groups);
        
        let mut state = Self {
            repo_root,
            repo_context,
            groups,
            selected_group_index: 0,
            diff_scroll_y: 0,
            diff_lines_cache: Vec::new(),
            verification,
            risk_summary,
            mode: TuiMode::Review,
            commit_message: commit_message.unwrap_or_default(),
            error_message: None,
            verification_logs: Vec::new(),
            verification_running: false,
            commit_success: None,
            commit_input_focused: true,
        };
        state.load_diff_cache();
        state
    }

    pub fn load_diff_cache(&mut self) {
        self.diff_lines_cache.clear();
        self.diff_scroll_y = 0;
        
        if self.groups.is_empty() {
            self.diff_lines_cache.push("No changes to show.".to_string());
            return;
        }

        let group = &self.groups[self.selected_group_index];
        if group.files.is_empty() {
            self.diff_lines_cache.push("No files in this group.".to_string());
            return;
        }

        let git = draft_core::git_adapter::GitCliAdapter::new(self.repo_root.clone());
        let opts = draft_core::git_adapter::DiffOptions {
            binary: true,
            paths: group.files.clone(),
        };

        match git.diff(opts) {
            Ok(diff) => {
                if diff.trim().is_empty() {
                    self.diff_lines_cache.push("No diff content (files might be newly added or untracked).".to_string());
                    // If untracked, let's see if we can show mock additions
                    for file_path in &group.files {
                        let full_path = self.repo_root.join(file_path);
                        if full_path.exists() && full_path.is_file() {
                            if let Ok(content) = std::fs::read_to_string(&full_path) {
                                self.diff_lines_cache.push(format!("+++ New File: {}", file_path.display()));
                                for line in content.lines() {
                                    self.diff_lines_cache.push(format!("+{}", line));
                                }
                            }
                        }
                    }
                } else {
                    for line in diff.lines() {
                        self.diff_lines_cache.push(line.to_string());
                    }
                }
            }
            Err(e) => {
                self.diff_lines_cache.push(format!("Error loading diff: {}", e));
            }
        }
    }

    pub fn toggle_selected_group(&mut self) {
        if !self.groups.is_empty() {
            self.groups[self.selected_group_index].included = !self.groups[self.selected_group_index].included;
            self.risk_summary = draft_core::risk_engine::RiskEngine::summarize(&self.groups);
        }
    }

    pub fn update_selection(&mut self, next: bool) {
        if self.groups.is_empty() {
            return;
        }
        if next {
            self.selected_group_index = (self.selected_group_index + 1) % self.groups.len();
        } else {
            if self.selected_group_index == 0 {
                self.selected_group_index = self.groups.len() - 1;
            } else {
                self.selected_group_index -= 1;
            }
        }
        self.load_diff_cache();
    }
}
