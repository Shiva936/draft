//! Shared status vocabulary for CLI, TUI, and AG-UI.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusKind {
    Ready,
    Running,
    Warning,
    Blocked,
    Pending,
    Linked,
    Refreshing,
    Defined,
    NeedsReview,
    Approved,
    Failed,
    Unknown,
}

impl StatusKind {
    pub fn label(self) -> &'static str {
        match self {
            StatusKind::Ready => "ready",
            StatusKind::Running => "running",
            StatusKind::Warning => "warning",
            StatusKind::Blocked => "blocked",
            StatusKind::Pending => "pending",
            StatusKind::Linked => "linked",
            StatusKind::Refreshing => "refreshing",
            StatusKind::Defined => "defined",
            StatusKind::NeedsReview => "needs review",
            StatusKind::Approved => "approved",
            StatusKind::Failed => "failed",
            StatusKind::Unknown => "unknown",
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            StatusKind::Ready | StatusKind::Approved => "✓",
            StatusKind::Running => "●",
            StatusKind::Warning => "!",
            StatusKind::Blocked | StatusKind::Failed => "×",
            StatusKind::Pending | StatusKind::Defined | StatusKind::Unknown => "◇",
            StatusKind::Linked => "⧉",
            StatusKind::Refreshing | StatusKind::NeedsReview => "↻",
        }
    }

    pub fn text(self) -> String {
        format!("{} {}", self.symbol(), self.label())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusDisplay {
    pub kind: StatusKind,
    pub label: String,
    pub symbol: String,
    pub text: String,
}

impl From<StatusKind> for StatusDisplay {
    fn from(kind: StatusKind) -> Self {
        StatusDisplay {
            kind,
            label: kind.label().to_string(),
            symbol: kind.symbol().to_string(),
            text: kind.text(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_include_symbol_and_label() {
        let display = StatusDisplay::from(StatusKind::NeedsReview);
        assert_eq!(display.symbol, "↻");
        assert!(display.text.contains("needs review"));
    }
}
