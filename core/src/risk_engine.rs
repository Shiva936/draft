use crate::errors::DraftError;
use crate::models::{ChangeGroup, ChangeGroupKind, RepoContext, RiskAssessment, RiskLevel, RiskReason};

pub struct RiskEngine;

impl RiskEngine {
    pub fn assess(groups: &[ChangeGroup], ctx: &RepoContext) -> Result<Vec<ChangeGroup>, DraftError> {
        let mut assessed_groups = Vec::new();
        
        for group in groups {
            let mut reasons = Vec::new();
            let mut level = RiskLevel::Low;
            
            if ctx.has_unmerged_conflicts {
                level = RiskLevel::Blocked;
                reasons.push(RiskReason {
                    code: "CONFLICTS".to_string(),
                    message: "Repository contains unresolved Git merge conflicts.".to_string(),
                    path: None,
                });
            }
            
            // Analyze each file in the group
            let mut touches_security = false;
            let mut lockfile_changed = false;
            
            for path in &group.files {
                let path_str = path.to_string_lossy().to_lowercase();
                let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("").to_lowercase();
                
                // Check auth/security/payment keywords
                let keywords = ["auth", "login", "security", "session", "payment", "billing", "stripe", "token", "secret", "key"];
                for kw in &keywords {
                    if path_str.contains(kw) && !path_str.contains("test") && !path_str.contains("mock") {
                        touches_security = true;
                        reasons.push(RiskReason {
                            code: "SECURITY_CODE".to_string(),
                            message: format!("Touched security-sensitive file: {}", path.display()),
                            path: Some(path.clone()),
                        });
                        break;
                    }
                }
                
                // Lockfile check
                let lockfiles = ["cargo.lock", "package-lock.json", "yarn.lock", "pnpm-lock.yaml", "go.sum", "poetry.lock"];
                if lockfiles.contains(&filename.as_str()) {
                    lockfile_changed = true;
                    reasons.push(RiskReason {
                        code: "LOCKFILE_CHANGED".to_string(),
                        message: format!("Dependency lockfile changed: {}", filename),
                        path: Some(path.clone()),
                    });
                }
            }
            
            // Check group kind risks
            match group.group_kind {
                ChangeGroupKind::MigrationChange => {
                    level = std::cmp::max(level, RiskLevel::High);
                    reasons.push(RiskReason {
                        code: "DB_MIGRATION".to_string(),
                        message: "Database schema migrations can impact production data schemas.".to_string(),
                        path: None,
                    });
                }
                ChangeGroupKind::BinaryChange => {
                    level = std::cmp::max(level, RiskLevel::High);
                    reasons.push(RiskReason {
                        code: "BINARY_ASSET".to_string(),
                        message: "Binary changes cannot be easily reviewed in text diffs.".to_string(),
                        path: None,
                    });
                }
                ChangeGroupKind::GeneratedChange => {
                    level = std::cmp::max(level, RiskLevel::High);
                    reasons.push(RiskReason {
                        code: "GENERATED_FILE".to_string(),
                        message: "Generated files contain automated modifications.".to_string(),
                        path: None,
                    });
                }
                ChangeGroupKind::RefactorLikeChange => {
                    level = std::cmp::max(level, RiskLevel::Medium);
                    reasons.push(RiskReason {
                        code: "REFACTOR".to_string(),
                        message: "Large refactor modifications could introduce regression errors.".to_string(),
                        path: None,
                    });
                }
                ChangeGroupKind::ConfigChange => {
                    level = std::cmp::max(level, RiskLevel::Medium);
                    reasons.push(RiskReason {
                        code: "CONFIG_CHANGE".to_string(),
                        message: "Configuration changes can alter runtime environments or build pipelines.".to_string(),
                        path: None,
                    });
                }
                ChangeGroupKind::SourceChange => {
                    level = std::cmp::max(level, RiskLevel::Medium);
                }
                ChangeGroupKind::DebugOrLoggingChange => {
                    level = std::cmp::max(level, RiskLevel::Low);
                    reasons.push(RiskReason {
                        code: "DEBUG_LOGS".to_string(),
                        message: "Contains temporary logging statements (e.g. println!, console.log).".to_string(),
                        path: None,
                    });
                }
                ChangeGroupKind::TestChange => {
                    level = std::cmp::max(level, RiskLevel::Low);
                }
                _ => {}
            }
            
            if touches_security {
                level = std::cmp::max(level, RiskLevel::High);
            }
            if lockfile_changed {
                level = std::cmp::max(level, RiskLevel::High);
            }
            
            // Group size risk
            if group.files.len() > 5 {
                level = std::cmp::max(level, RiskLevel::High);
                reasons.push(RiskReason {
                    code: "MANY_FILES".to_string(),
                    message: format!("Contains {} files, making this group large.", group.files.len()),
                    path: None,
                });
            }
            
            // Defaults
            if level == RiskLevel::Medium && reasons.is_empty() {
                reasons.push(RiskReason {
                    code: "SOURCE_CHANGE".to_string(),
                    message: "Standard application source code modifications.".to_string(),
                    path: None,
                });
            }
            if level == RiskLevel::Low && reasons.is_empty() {
                reasons.push(RiskReason {
                    code: "LOW_RISK".to_string(),
                    message: "Low-impact changes (tests or documentation).".to_string(),
                    path: None,
                });
            }

            let mut group_assessed = group.clone();
            group_assessed.risk = RiskAssessment { level, reasons };
            assessed_groups.push(group_assessed);
        }
        
        Ok(assessed_groups)
    }

    pub fn summarize(groups: &[ChangeGroup]) -> RiskAssessment {
        if groups.is_empty() {
            return RiskAssessment {
                level: RiskLevel::Low,
                reasons: vec![RiskReason {
                    code: "CLEAN".to_string(),
                    message: "No changes detected in working tree.".to_string(),
                    path: None,
                }],
            };
        }
        
        let mut max_level = RiskLevel::Low;
        let mut reasons = Vec::new();
        
        for group in groups {
            if group.included {
                max_level = std::cmp::max(max_level, group.risk.level);
                for r in &group.risk.reasons {
                    if !reasons.contains(r) {
                        reasons.push(r.clone());
                    }
                }
            }
        }
        
        RiskAssessment {
            level: max_level,
            reasons,
        }
    }
}
