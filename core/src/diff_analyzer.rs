use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use crate::errors::DraftError;
use crate::models::{DiffHunk, DiffLine, FileChange, FileStatus};

pub struct DiffAnalyzer;

impl DiffAnalyzer {
    pub fn analyze(repo_root: &Path, diff_text: &str, status_text: &str) -> Result<Vec<FileChange>, DraftError> {
        let mut status_map = HashMap::new();
        
        // 1. Parse porcelain status
        for line in status_text.lines() {
            if line.len() < 4 {
                continue;
            }
            let code = &line[0..2];
            let path_part = &line[3..];
            
            let clean_path = |p: &str| -> PathBuf {
                let trimmed = p.trim();
                let unquoted = if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() > 1 {
                    &trimmed[1..trimmed.len()-1]
                } else {
                    trimmed
                };
                PathBuf::from(unquoted)
            };

            let (status, path) = if code.contains('R') {
                if let Some(pos) = path_part.find(" -> ") {
                    let old_p = clean_path(&path_part[..pos]);
                    let new_p = clean_path(&path_part[pos + 4..]);
                    (FileStatus::Renamed { old_path: old_p }, new_p)
                } else {
                    (FileStatus::Modified, clean_path(path_part))
                }
            } else if code == "??" {
                (FileStatus::Untracked, clean_path(path_part))
            } else if code.contains('A') {
                (FileStatus::Added, clean_path(path_part))
            } else if code.contains('D') {
                (FileStatus::Deleted, clean_path(path_part))
            } else {
                (FileStatus::Modified, clean_path(path_part))
            };

            status_map.insert(path.clone(), status);
        }

        // 2. Parse diff_text
        let mut file_changes = Vec::new();
        let mut current_file: Option<FileChange> = None;
        let mut current_hunk: Option<DiffHunk> = None;

        for line in diff_text.lines() {
            if line.starts_with("diff --git ") {
                // Save previous hunk and file
                if let Some(h) = current_hunk.take() {
                    if let Some(ref mut file) = current_file {
                        file.hunks.push(h);
                    }
                }
                if let Some(file) = current_file.take() {
                    file_changes.push(file);
                }

                // Parse new file path
                let path = if let Some(b_idx) = line.rfind(" b/") {
                    let p = &line[b_idx + 3..];
                    let unquoted = if p.starts_with('"') && p.ends_with('"') && p.len() > 1 {
                        &p[1..p.len()-1]
                    } else {
                        p
                    };
                    PathBuf::from(unquoted)
                } else {
                    PathBuf::from("")
                };

                let status = status_map.get(&path).cloned().unwrap_or(FileStatus::Modified);

                current_file = Some(FileChange {
                    path,
                    status,
                    additions: 0,
                    deletions: 0,
                    is_binary: false,
                    hunks: Vec::new(),
                });
            } else if line.starts_with("Binary files ") && line.contains("differ") {
                if let Some(ref mut file) = current_file {
                    file.is_binary = true;
                }
            } else if line.starts_with("@@ ") {
                // Save previous hunk
                if let Some(h) = current_hunk.take() {
                    if let Some(ref mut file) = current_file {
                        file.hunks.push(h);
                    }
                }

                // Parse hunk header
                let parts: Vec<&str> = line.split("@@").collect();
                if parts.len() >= 3 {
                    let header_content = parts[1].trim();
                    let subparts: Vec<&str> = header_content.split_whitespace().collect();
                    if subparts.len() >= 2 {
                        let parse_part = |p: &str| -> (usize, usize) {
                            let clean = &p[1..];
                            if let Some(comma) = clean.find(',') {
                                let start = clean[..comma].parse().unwrap_or(0);
                                let count = clean[comma+1..].parse().unwrap_or(0);
                                (start, count)
                            } else {
                                let start = clean.parse().unwrap_or(0);
                                (start, 1)
                            }
                        };
                        let (old_start, old_lines) = parse_part(subparts[0]);
                        let (new_start, new_lines) = parse_part(subparts[1]);

                        current_hunk = Some(DiffHunk {
                            old_start,
                            old_lines,
                            new_start,
                            new_lines,
                            header: line.to_string(),
                            lines: Vec::new(),
                        });
                    }
                }
            } else if let Some(ref mut hunk) = current_hunk {
                if line.starts_with('+') {
                    hunk.lines.push(DiffLine::Added(line[1..].to_string()));
                    if let Some(ref mut file) = current_file {
                        file.additions += 1;
                    }
                } else if line.starts_with('-') {
                    hunk.lines.push(DiffLine::Removed(line[1..].to_string()));
                    if let Some(ref mut file) = current_file {
                        file.deletions += 1;
                    }
                } else if line.starts_with(' ') {
                    hunk.lines.push(DiffLine::Context(line[1..].to_string()));
                }
            }
        }

        // Save last hunk and file
        if let Some(h) = current_hunk.take() {
            if let Some(ref mut file) = current_file {
                file.hunks.push(h);
            }
        }
        if let Some(file) = current_file.take() {
            file_changes.push(file);
        }

        // Create a lookup for parsed changes
        let mut parsed_map: HashMap<PathBuf, FileChange> = file_changes
            .into_iter()
            .map(|f| (f.path.clone(), f))
            .collect();

        // 3. For any untracked or status-registered file not present in diff_text, add it
        for (path, status) in status_map {
            if !parsed_map.contains_key(&path) {
                let mut change = FileChange {
                    path: path.clone(),
                    status: status.clone(),
                    additions: 0,
                    deletions: 0,
                    is_binary: false,
                    hunks: Vec::new(),
                };

                if status == FileStatus::Untracked {
                    let full_path = repo_root.join(&path);
                    if full_path.exists() && full_path.is_file() {
                        if let Ok(content) = fs::read_to_string(&full_path) {
                            let lines: Vec<DiffLine> = content
                                .lines()
                                .map(|l| DiffLine::Added(l.to_string()))
                                .collect();
                            
                            change.additions = lines.len();
                            change.hunks.push(DiffHunk {
                                old_start: 0,
                                old_lines: 0,
                                new_start: 1,
                                new_lines: lines.len(),
                                header: format!("@@ -0,0 +1,{} @@", lines.len()),
                                lines,
                            });
                        } else {
                            change.is_binary = true; // assume binary if read fails
                        }
                    }
                }
                parsed_map.insert(path, change);
            }
        }

        Ok(parsed_map.into_values().collect())
    }
}
