//! Map `git status --porcelain=v1` to provider-neutral [`ProviderStatus`].

use draft_core::common::WorkspacePath;
use draft_core::vcs::errors::ProviderError;
use draft_core::vcs::types::{FileStatus, ProviderStatus, StatusEntry};

use crate::command::GitCommand;
use crate::parse::{is_conflict_code, unquote};

pub fn status(git: &GitCommand) -> Result<ProviderStatus, ProviderError> {
    let text = git.status_porcelain()?;
    Ok(parse_status(&text))
}

pub fn parse_status(text: &str) -> ProviderStatus {
    let mut entries = Vec::new();
    let mut has_staged = false;
    let mut has_conflicts = false;

    for line in text.lines() {
        if line.len() < 3 {
            continue;
        }
        let code = &line[0..2];
        let rest = &line[3..];

        if is_conflict_code(code) {
            has_conflicts = true;
            entries.push(StatusEntry {
                path: WorkspacePath::new(unquote(rest)),
                status: FileStatus::Conflicted,
                old_path: None,
            });
            continue;
        }

        // Index (staged) column is the first char; non-space, non-? means staged.
        let index_char = line.chars().next().unwrap_or(' ');
        if index_char != ' ' && index_char != '?' {
            has_staged = true;
        }

        let (status, path, old_path) = if code.contains('R') {
            if let Some(pos) = rest.find(" -> ") {
                (
                    FileStatus::Renamed,
                    WorkspacePath::new(unquote(&rest[pos + 4..])),
                    Some(WorkspacePath::new(unquote(&rest[..pos]))),
                )
            } else {
                (
                    FileStatus::Modified,
                    WorkspacePath::new(unquote(rest)),
                    None,
                )
            }
        } else if code.contains('C') {
            if let Some(pos) = rest.find(" -> ") {
                (
                    FileStatus::Copied,
                    WorkspacePath::new(unquote(&rest[pos + 4..])),
                    Some(WorkspacePath::new(unquote(&rest[..pos]))),
                )
            } else {
                (FileStatus::Copied, WorkspacePath::new(unquote(rest)), None)
            }
        } else if code == "??" {
            (
                FileStatus::Untracked,
                WorkspacePath::new(unquote(rest)),
                None,
            )
        } else if code.contains('A') {
            (FileStatus::Added, WorkspacePath::new(unquote(rest)), None)
        } else if code.contains('D') {
            (FileStatus::Deleted, WorkspacePath::new(unquote(rest)), None)
        } else if code.contains('T') {
            (
                FileStatus::TypeChanged,
                WorkspacePath::new(unquote(rest)),
                None,
            )
        } else {
            (
                FileStatus::Modified,
                WorkspacePath::new(unquote(rest)),
                None,
            )
        };

        entries.push(StatusEntry {
            path,
            status,
            old_path,
        });
    }

    ProviderStatus {
        entries,
        has_staged_changes: has_staged,
        has_conflicts,
    }
}
