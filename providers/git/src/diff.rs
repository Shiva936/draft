//! Map `git diff` output to the provider-neutral [`ProviderDelta`].

use std::collections::HashMap;

use draft_core::common::WorkspacePath;
use draft_core::vcs::errors::ProviderError;
use draft_core::vcs::types::{
    DiffHunk, DiffInput, DiffLine, DiffStats, FileDelta, FileStatus, ProviderDelta,
    ProviderRevisionId,
};

use crate::command::{GitCommand, ZERO_OID};
use crate::parse::unquote;
use crate::status::parse_status;

/// Files whose changed-line count exceeds this are summarized (hunks dropped,
/// counts retained) to avoid holding huge diffs in memory (NFRD §6.4).
const SUMMARIZE_LINE_THRESHOLD: usize = 2000;

pub fn diff(git: &GitCommand, input: DiffInput) -> Result<ProviderDelta, ProviderError> {
    let mut args: Vec<String> = vec!["diff".to_string(), "--binary".to_string()];
    if let DiffInput::Paths(paths) = &input {
        args.push("--".to_string());
        for p in paths {
            args.push(p.as_str().to_string());
        }
    }
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let diff_text = git.run(&arg_refs)?;
    let status_text = git.status_porcelain()?;

    let head = git.current_head().unwrap_or_else(|_| ZERO_OID.to_string());
    let base = if head == ZERO_OID {
        None
    } else {
        Some(ProviderRevisionId::new(head))
    };

    let files = build_file_deltas(&git.cwd, &diff_text, &status_text);
    let stats = compute_stats(&files);
    Ok(ProviderDelta { base, files, stats })
}

fn compute_stats(files: &[FileDelta]) -> DiffStats {
    let mut stats = DiffStats {
        files_changed: files.len(),
        ..Default::default()
    };
    for f in files {
        stats.additions += f.additions;
        stats.deletions += f.deletions;
        if f.binary {
            stats.binary_files += 1;
        }
    }
    stats
}

fn build_file_deltas(
    repo_root: &std::path::Path,
    diff_text: &str,
    status_text: &str,
) -> Vec<FileDelta> {
    // Status gives us authoritative file statuses (and renames/untracked).
    let status = parse_status(status_text);
    let mut status_map: HashMap<String, (FileStatus, Option<WorkspacePath>)> = HashMap::new();
    for e in &status.entries {
        status_map.insert(e.path.as_str().to_string(), (e.status, e.old_path.clone()));
    }

    let mut parsed: HashMap<String, FileDelta> = HashMap::new();
    let mut current: Option<FileDelta> = None;
    let mut current_hunk: Option<DiffHunk> = None;

    let flush_hunk = |cur: &mut Option<FileDelta>, hunk: &mut Option<DiffHunk>| {
        if let Some(h) = hunk.take() {
            if let Some(f) = cur.as_mut() {
                f.hunks.push(h);
            }
        }
    };

    for line in diff_text.lines() {
        if line.starts_with("diff --git ") {
            flush_hunk(&mut current, &mut current_hunk);
            if let Some(f) = current.take() {
                parsed.insert(f.path.as_str().to_string(), f);
            }
            let path = parse_b_path(line);
            let (status, old_path) = status_map
                .get(&path)
                .cloned()
                .unwrap_or((FileStatus::Modified, None));
            current = Some(FileDelta {
                path: WorkspacePath::new(path),
                old_path,
                status,
                hunks: Vec::new(),
                binary: false,
                summarized: false,
                additions: 0,
                deletions: 0,
            });
        } else if line.starts_with("Binary files ") && line.contains("differ") {
            if let Some(f) = current.as_mut() {
                f.binary = true;
            }
        } else if line.starts_with("@@ ") {
            flush_hunk(&mut current, &mut current_hunk);
            if let Some(h) = parse_hunk_header(line) {
                current_hunk = Some(h);
            }
        } else if let Some(h) = current_hunk.as_mut() {
            if let Some(rest) = line.strip_prefix('+') {
                h.lines.push(DiffLine::Added(rest.to_string()));
                if let Some(f) = current.as_mut() {
                    f.additions += 1;
                }
            } else if let Some(rest) = line.strip_prefix('-') {
                h.lines.push(DiffLine::Removed(rest.to_string()));
                if let Some(f) = current.as_mut() {
                    f.deletions += 1;
                }
            } else if let Some(rest) = line.strip_prefix(' ') {
                h.lines.push(DiffLine::Context(rest.to_string()));
            }
        }
    }
    flush_hunk(&mut current, &mut current_hunk);
    if let Some(f) = current.take() {
        parsed.insert(f.path.as_str().to_string(), f);
    }

    // Add status-only entries not present in the textual diff (e.g. untracked).
    for (path, (status, old_path)) in &status_map {
        if parsed.contains_key(path) {
            continue;
        }
        let mut delta = FileDelta {
            path: WorkspacePath::new(path.clone()),
            old_path: old_path.clone(),
            status: *status,
            hunks: Vec::new(),
            binary: false,
            summarized: false,
            additions: 0,
            deletions: 0,
        };
        if *status == FileStatus::Untracked {
            let full = repo_root.join(path);
            if full.is_file() {
                match std::fs::read_to_string(&full) {
                    Ok(content) => {
                        let lines: Vec<DiffLine> = content
                            .lines()
                            .map(|l| DiffLine::Added(l.to_string()))
                            .collect();
                        delta.additions = lines.len();
                        delta.hunks.push(DiffHunk {
                            old_start: 0,
                            old_lines: 0,
                            new_start: 1,
                            new_lines: lines.len(),
                            header: format!("@@ -0,0 +1,{} @@", lines.len()),
                            lines,
                        });
                    }
                    Err(_) => delta.binary = true,
                }
            }
        }
        parsed.insert(path.clone(), delta);
    }

    let mut out: Vec<FileDelta> = parsed.into_values().collect();
    for f in &mut out {
        // Summarize oversized text files: keep counts, drop content.
        if !f.binary && (f.additions + f.deletions) > SUMMARIZE_LINE_THRESHOLD {
            f.summarized = true;
            f.hunks.clear();
        }
        if f.binary {
            f.summarized = true;
            f.hunks.clear();
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn parse_b_path(line: &str) -> String {
    if let Some(idx) = line.rfind(" b/") {
        unquote(&line[idx + 3..])
    } else {
        String::new()
    }
}

fn parse_hunk_header(line: &str) -> Option<DiffHunk> {
    let parts: Vec<&str> = line.split("@@").collect();
    if parts.len() < 3 {
        return None;
    }
    let sub: Vec<&str> = parts[1].split_whitespace().collect();
    if sub.len() < 2 {
        return None;
    }
    let parse_part = |p: &str| -> (usize, usize) {
        let clean = &p[1..]; // skip leading - or +
        if let Some(c) = clean.find(',') {
            (
                clean[..c].parse().unwrap_or(0),
                clean[c + 1..].parse().unwrap_or(0),
            )
        } else {
            (clean.parse().unwrap_or(0), 1)
        }
    };
    let (old_start, old_lines) = parse_part(sub[0]);
    let (new_start, new_lines) = parse_part(sub[1]);
    Some(DiffHunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        header: line.to_string(),
        lines: Vec::new(),
    })
}
