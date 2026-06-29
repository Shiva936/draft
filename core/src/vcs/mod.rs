//! Provider-neutral version-control abstraction.
//!
//! This module is the heart of Draft's provider neutrality. It defines:
//! - opaque provider-native identifiers and neutral value types (`types`)
//! - provider capability declarations (`capabilities`)
//! - the [`VcsProvider`]/[`VcsRepository`] traits (`traits`)
//! - detection results and ranking (`detection`)
//! - the [`ProviderRegistry`] (`registry`)
//! - the structured [`ProviderError`] model (`errors`)
//!
//! Nothing here references Git or any specific provider.

pub mod capabilities;
pub mod detection;
pub mod errors;
pub mod experimental;
pub mod registry;
pub mod traits;
pub mod types;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use capabilities::ProviderCapabilities;
pub use detection::{DetectionConfidence, ProviderDetection};
pub use errors::{ProviderError, ProviderErrorKind};
pub use registry::{ProviderInfo, ProviderRegistry, ProviderSelection};
pub use traits::{VcsProvider, VcsRepository};
pub use types::*;
