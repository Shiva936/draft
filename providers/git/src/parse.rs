//! Shared parsing helpers for Git porcelain output.

/// Strip surrounding quotes that git adds for paths containing special chars.
pub fn unquote(p: &str) -> String {
    let t = p.trim();
    if t.starts_with('"') && t.ends_with('"') && t.len() > 1 {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

/// Porcelain status codes that indicate an unmerged (conflicted) entry.
pub const CONFLICT_CODES: &[&str] = &["DD", "AU", "UD", "UA", "DU", "AA", "UU"];

pub fn is_conflict_code(code: &str) -> bool {
    CONFLICT_CODES.contains(&code)
}
