//! Provider registry: registration, listing, and detection-based selection.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::capabilities::ProviderCapabilities;
use super::detection::{DetectionConfidence, ProviderDetection};
use super::errors::{ProviderError, ProviderErrorKind};
use super::traits::VcsProvider;
use super::types::ProviderId;

/// Summary information about a registered provider, for `draft provider list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub id: ProviderId,
    pub name: String,
    pub description: String,
    pub experimental: bool,
    pub capabilities: ProviderCapabilities,
}

/// The chosen provider for a path together with its detection result.
#[derive(Clone)]
pub struct ProviderSelection {
    pub provider: Arc<dyn VcsProvider>,
    pub detection: ProviderDetection,
}

impl std::fmt::Debug for ProviderSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderSelection")
            .field("provider", &self.provider.id())
            .field("detection", &self.detection)
            .finish()
    }
}

/// Holds the set of providers available to a client (CLI, daemon, tests).
///
/// Core deliberately ships an **empty** registry; clients register concrete
/// providers (this keeps `core` free of any dependency on `providers/*`).
#[derive(Clone, Default)]
pub struct ProviderRegistry {
    providers: Vec<Arc<dyn VcsProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        ProviderRegistry {
            providers: Vec::new(),
        }
    }

    pub fn register(&mut self, provider: Arc<dyn VcsProvider>) {
        self.providers.push(provider);
    }

    pub fn with(mut self, provider: Arc<dyn VcsProvider>) -> Self {
        self.register(provider);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    pub fn providers(&self) -> Vec<ProviderInfo> {
        self.providers
            .iter()
            .map(|p| ProviderInfo {
                id: p.id(),
                name: p.name().to_string(),
                description: p.description().to_string(),
                experimental: p.is_experimental(),
                capabilities: p.capabilities(),
            })
            .collect()
    }

    pub fn get(&self, id: &ProviderId) -> Option<Arc<dyn VcsProvider>> {
        self.providers.iter().find(|p| &p.id() == id).cloned()
    }

    /// Detect the best provider for `path`.
    ///
    /// Ranking is by `DetectionConfidence`. If two providers tie at the highest
    /// non-`None` confidence, returns `ProviderErrorKind::Ambiguous`.
    pub fn detect(&self, path: &Path) -> Result<ProviderSelection, ProviderError> {
        let mut results: Vec<(Arc<dyn VcsProvider>, ProviderDetection)> = Vec::new();
        for p in &self.providers {
            match p.detect(path) {
                Ok(det) if det.confidence != DetectionConfidence::None => {
                    results.push((p.clone(), det));
                }
                Ok(_) => {}
                // A provider failing to detect must not abort overall detection.
                Err(_) => {}
            }
        }

        if results.is_empty() {
            return Err(ProviderError::new(
                ProviderErrorKind::NotDetected,
                "No provider could be detected for this path.",
            )
            .with_suggestion(
                "Run inside a supported repository, or use the filesystem provider.",
            ));
        }

        results.sort_by_key(|(_, detection)| std::cmp::Reverse(detection.confidence));
        let best_conf = results[0].1.confidence;
        let tied: Vec<_> = results
            .iter()
            .filter(|(_, d)| d.confidence == best_conf)
            .collect();

        if tied.len() > 1 {
            let names: Vec<String> = tied.iter().map(|(p, _)| p.name().to_string()).collect();
            return Err(ProviderError::new(
                ProviderErrorKind::Ambiguous,
                format!(
                    "Multiple providers matched with equal confidence: {}",
                    names.join(", ")
                ),
            )
            .with_suggestion(
                "Select a provider explicitly with `draft workspace init --provider <id>`.",
            ));
        }

        let (provider, detection) = results.into_iter().next().unwrap();
        Ok(ProviderSelection {
            provider,
            detection,
        })
    }
}
