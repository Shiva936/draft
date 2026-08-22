//! Shared design tokens exported by core surfaces.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignTokens {
    pub accent: String,
    pub success: String,
    pub warning: String,
    pub danger: String,
    pub surface: String,
    pub text: String,
    pub muted_text: String,
    pub focus_ring: String,
    pub radius_sm: u8,
    pub radius_md: u8,
    pub spacing_sm: u8,
    pub spacing_md: u8,
}

impl Default for DesignTokens {
    fn default() -> Self {
        DesignTokens {
            accent: "#2563eb".into(),
            success: "#15803d".into(),
            warning: "#b45309".into(),
            danger: "#b91c1c".into(),
            surface: "#ffffff".into(),
            text: "#111827".into(),
            muted_text: "#4b5563".into(),
            focus_ring: "#0ea5e9".into(),
            radius_sm: 4,
            radius_md: 8,
            spacing_sm: 8,
            spacing_md: 12,
        }
    }
}

pub fn tokens() -> DesignTokens {
    DesignTokens::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agui_tokens_match_core_tokens() {
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("core crate has workspace parent");
        let path = workspace.join("services/agui/web/src/tokens.json");
        let exported: DesignTokens =
            serde_json::from_slice(&std::fs::read(&path).expect("AG-UI tokens.json exists"))
                .expect("AG-UI tokens.json parses");
        assert_eq!(exported, tokens());
    }
}
