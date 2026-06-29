use uuid::Uuid;

use crate::errors::DraftError;
use crate::models::{ChangeGroup, ChangeGroupKind, FileChange, HunkRef, RiskAssessment, RiskLevel};

pub struct ChangeGrouper;

impl ChangeGrouper {
    pub fn group(changes: &[FileChange]) -> Result<Vec<ChangeGroup>, DraftError> {
        if changes.is_empty() {
            return Ok(Vec::new());
        }

        // Group files by their classified kind
        let mut grouped_files: std::collections::HashMap<ChangeGroupKind, Vec<FileChange>> = std::collections::HashMap::new();

        for change in changes {
            let kind = classify_change(change);
            grouped_files.entry(kind).or_default().push(change.clone());
        }

        let mut groups = Vec::new();

        // Order the kinds logically (e.g. Source, Test, Migration, Dependency, etc.)
        let ordered_kinds = [
            ChangeGroupKind::SourceChange,
            ChangeGroupKind::TestChange,
            ChangeGroupKind::MigrationChange,
            ChangeGroupKind::DependencyChange,
            ChangeGroupKind::ConfigChange,
            ChangeGroupKind::RefactorLikeChange,
            ChangeGroupKind::DebugOrLoggingChange,
            ChangeGroupKind::GeneratedChange,
            ChangeGroupKind::BinaryChange,
            ChangeGroupKind::Unknown,
        ];

        for kind in &ordered_kinds {
            if let Some(files) = grouped_files.remove(kind) {
                let title = get_group_title(*kind);
                let description = get_group_description(*kind, &files);
                
                let mut group_files = Vec::new();
                let mut group_hunks = Vec::new();

                for file in &files {
                    group_files.push(file.path.clone());
                    for hunk_idx in 0..file.hunks.len() {
                        group_hunks.push(HunkRef {
                            file_path: file.path.clone(),
                            hunk_index: hunk_idx,
                        });
                    }
                }

                groups.push(ChangeGroup {
                    group_id: Uuid::new_v4().to_string(),
                    title,
                    description: Some(description),
                    files: group_files,
                    hunks: group_hunks,
                    risk: RiskAssessment {
                        level: RiskLevel::Low, // To be assessed by RiskEngine
                        reasons: Vec::new(),
                    },
                    group_kind: *kind,
                    included: true, // Included by default
                });
            }
        }

        // Catch any remaining kinds not in ordered list
        for (kind, files) in grouped_files {
            let title = get_group_title(kind);
            let description = get_group_description(kind, &files);
            
            let mut group_files = Vec::new();
            let mut group_hunks = Vec::new();

            for file in &files {
                group_files.push(file.path.clone());
                for hunk_idx in 0..file.hunks.len() {
                    group_hunks.push(HunkRef {
                        file_path: file.path.clone(),
                        hunk_index: hunk_idx,
                    });
                }
            }

            groups.push(ChangeGroup {
                group_id: Uuid::new_v4().to_string(),
                title,
                description: Some(description),
                files: group_files,
                hunks: group_hunks,
                risk: RiskAssessment {
                    level: RiskLevel::Low,
                    reasons: Vec::new(),
                },
                group_kind: kind,
                included: true,
            });
        }

        Ok(groups)
    }
}

fn classify_change(change: &FileChange) -> ChangeGroupKind {
    if change.is_binary {
        return ChangeGroupKind::BinaryChange;
    }
    
    let path_str = change.path.to_string_lossy().to_lowercase();
    
    // 1. Database Migrations
    if path_str.contains("migrations/") || path_str.contains("schema.sql") || path_str.contains("db/") || path_str.contains("prisma/migrations/") {
        return ChangeGroupKind::MigrationChange;
    }
    
    // 2. Tests
    if path_str.contains("tests/") || path_str.contains("test/") || path_str.contains("_test.") || path_str.contains(".test.") || path_str.contains("_spec.") || path_str.contains(".spec.") {
        return ChangeGroupKind::TestChange;
    }
    
    // 3. Dependencies
    let filename = change.path.file_name().and_then(|f| f.to_str()).unwrap_or("").to_lowercase();
    let dep_files = [
        "cargo.toml", "cargo.lock", "package.json", "package-lock.json", 
        "yarn.lock", "pnpm-lock.yaml", "go.mod", "go.sum", 
        "requirements.txt", "pyproject.toml", "poetry.lock"
    ];
    if dep_files.contains(&filename.as_str()) {
        return ChangeGroupKind::DependencyChange;
    }
    
    // 4. Config
    if filename.ends_with(".toml") || filename.ends_with(".yaml") || filename.ends_with(".yml") || filename.ends_with(".json") || path_str.contains(".github/") || filename == "dockerfile" || filename.starts_with("docker-compose.") {
        return ChangeGroupKind::ConfigChange;
    }
    
    // 5. Generated
    if path_str.contains("generated/") || path_str.contains("dist/") || path_str.contains("build/") || path_str.contains("target/") || path_str.contains(".generated.") {
        return ChangeGroupKind::GeneratedChange;
    }
    
    // 6. Binary extension check
    let bin_exts = ["png", "jpg", "jpeg", "gif", "ico", "pdf", "bin", "zip", "tar", "gz", "exe", "dll", "so", "dylib"];
    if let Some(ext) = change.path.extension().and_then(|e| e.to_str()) {
        if bin_exts.contains(&ext.to_lowercase().as_str()) {
            return ChangeGroupKind::BinaryChange;
        }
    }
    
    // 7. Debug/logging
    let mut has_debug = false;
    for hunk in &change.hunks {
        for line in &hunk.lines {
            if let crate::models::DiffLine::Added(content) = line {
                let lower = content.to_lowercase();
                if lower.contains("console.log") || lower.contains("println!") || lower.contains("dbg!") || lower.contains("log.info") || lower.contains("logger.info") {
                    has_debug = true;
                    break;
                }
            }
        }
    }
    if has_debug {
        return ChangeGroupKind::DebugOrLoggingChange;
    }
    
    // 8. Refactor-like
    if change.additions > 50 && change.deletions > 50 {
        return ChangeGroupKind::RefactorLikeChange;
    }
    
    // Default
    ChangeGroupKind::SourceChange
}

fn get_group_title(kind: ChangeGroupKind) -> String {
    match kind {
        ChangeGroupKind::SourceChange => "Source Changes".to_string(),
        ChangeGroupKind::TestChange => "Test Changes".to_string(),
        ChangeGroupKind::ConfigChange => "Configuration Changes".to_string(),
        ChangeGroupKind::DependencyChange => "Dependency Changes".to_string(),
        ChangeGroupKind::MigrationChange => "Database Migrations".to_string(),
        ChangeGroupKind::GeneratedChange => "Generated Files".to_string(),
        ChangeGroupKind::BinaryChange => "Binary Assets".to_string(),
        ChangeGroupKind::RefactorLikeChange => "Refactorings".to_string(),
        ChangeGroupKind::DebugOrLoggingChange => "Debug/Logging Tweaks".to_string(),
        ChangeGroupKind::Unknown => "Uncategorized Changes".to_string(),
    }
}

fn get_group_description(kind: ChangeGroupKind, files: &[FileChange]) -> String {
    let file_count = files.len();
    let total_additions: usize = files.iter().map(|f| f.additions).sum();
    let total_deletions: usize = files.iter().map(|f| f.deletions).sum();

    match kind {
        ChangeGroupKind::SourceChange => format!("{} source files modified (+{}, -{})", file_count, total_additions, total_deletions),
        ChangeGroupKind::TestChange => format!("{} test files modified (+{}, -{})", file_count, total_additions, total_deletions),
        ChangeGroupKind::ConfigChange => format!("{} configuration files changed (+{}, -{})", file_count, total_additions, total_deletions),
        ChangeGroupKind::DependencyChange => format!("{} dependency manifest/lockfiles changed", file_count),
        ChangeGroupKind::MigrationChange => format!("{} schema migration files", file_count),
        ChangeGroupKind::GeneratedChange => format!("{} build/generated files", file_count),
        ChangeGroupKind::BinaryChange => format!("{} binary assets changed", file_count),
        ChangeGroupKind::RefactorLikeChange => format!("Large refactor modifications in {} files (+{}, -{})", file_count, total_additions, total_deletions),
        ChangeGroupKind::DebugOrLoggingChange => format!("{} files with console/print logging additions", file_count),
        ChangeGroupKind::Unknown => format!("{} uncategorized changes (+{}, -{})", file_count, total_additions, total_deletions),
    }
}
