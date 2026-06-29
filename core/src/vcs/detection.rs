//! Provider detection results and confidence ranking.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::types::ProviderId;

/// How confident a provider is that it owns a given path.
///
/// Ordered: `Exact > High > Medium > Low > None` (Blueprint §7.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DetectionConfidence {
    None,
    Low,
    Medium,
    High,
    Exact,
}

/// A single provider's detection result for a path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDetection {
    pub provider_id: ProviderId,
    pub root: PathBuf,
    pub confidence: DetectionConfidence,
    pub reason: String,
}

impl ProviderDetection {
    pub fn none(provider_id: ProviderId, root: PathBuf) -> Self {
        ProviderDetection {
            provider_id,
            root,
            confidence: DetectionConfidence::None,
            reason: "not detected".to_string(),
        }
    }
}
