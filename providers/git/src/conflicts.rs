//! Git conflict detection mapped to the neutral [`ConflictSet`].

use draft_core::common::WorkspacePath;
use draft_core::vcs::errors::ProviderError;
use draft_core::vcs::types::{Conflict, ConflictKind, ConflictSet, FileStatus};

use crate::command::GitCommand;
use crate::status::status;

pub fn conflicts(git: &GitCommand) -> Result<ConflictSet, ProviderError> {
    let st = status(git)?;
    let mut set = ConflictSet::default();
    for entry in st.entries {
        if entry.status == FileStatus::Conflicted {
            set.conflicts.push(Conflict {
                path: entry.path,
                kind: ConflictKind::Provider,
                message: "git reports an unmerged (conflicted) path".to_string(),
            });
        }
    }

    // Defensive: also scan tracked text files for conflict markers, since a user
    // may have manually pasted them.
    if set.conflicts.is_empty() {
        let toplevel = git.toplevel().unwrap_or_else(|_| git.cwd.clone());
        for line in git.status_porcelain()?.lines() {
            if line.len() < 3 {
                continue;
            }
            let path = crate::parse::unquote(&line[3..]);
            let full = toplevel.join(&path);
            if full.is_file() {
                if let Ok(content) = std::fs::read_to_string(&full) {
                    if content.contains("<<<<<<<")
                        && content.contains("=======")
                        && content.contains(">>>>>>>")
                    {
                        set.conflicts.push(Conflict {
                            path: WorkspacePath::new(path),
                            kind: ConflictKind::Provider,
                            message: "conflict markers found in file".to_string(),
                        });
                    }
                }
            }
        }
    }

    Ok(set)
}
