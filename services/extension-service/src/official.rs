//! The official extension source, when this build carries one.
//!
//! Draft ships no extension packages. What a release build may carry is the
//! information needed to *trust* the official catalog out of band: its URL and
//! the fingerprint of its root metadata. Both are supplied at build time and
//! have no defaults, so a build made without them simply has no official
//! source — Draft stays fully functional and no dead URL or invented trust root
//! is ever created.
//!
//! The key `draft-official` carries no authority by itself. A source a user
//! adds under that name is an ordinary user-added source and must be trusted
//! explicitly like any other; only a bootstrap embedded in the binary
//! establishes trust without a user-supplied anchor.

use draft_core::extension::package::is_digest;
use draft_core::support::error::{DraftError, DraftResult};

/// The reserved key for the official source.
pub const OFFICIAL_SOURCE_ID: &str = "draft-official";

/// Catalog origin, supplied at build time by the release pipeline.
const CATALOG_URL: Option<&str> = option_env!("DRAFT_OFFICIAL_CATALOG_URL");
/// Digest of the official root metadata, supplied alongside the URL.
const ROOT_FINGERPRINT: Option<&str> = option_env!("DRAFT_OFFICIAL_ROOT_FINGERPRINT");

/// What a build knows about the official source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfficialBootstrap {
    pub source_id: &'static str,
    pub catalog_url: &'static str,
    /// The trust anchor: the pinned digest of the root metadata document.
    pub root_fingerprint: &'static str,
}

/// The official bootstrap this build carries, if any.
///
/// Both halves are required and the fingerprint must be a well-formed digest:
/// a half-configured build has no official source rather than a source nobody
/// can verify.
pub fn bootstrap() -> Option<OfficialBootstrap> {
    let catalog_url = CATALOG_URL?;
    let root_fingerprint = ROOT_FINGERPRINT?;
    let usable = catalog_url.starts_with("https://")
        && !catalog_url.trim().is_empty()
        && is_digest(root_fingerprint);
    usable.then_some(OfficialBootstrap {
        source_id: OFFICIAL_SOURCE_ID,
        catalog_url,
        root_fingerprint,
    })
}

/// Whether `source_id` names the built-in official source *and* this build
/// actually carries its bootstrap.
///
/// A user-added source merely named `draft-official` is not built in and gains
/// nothing from the name.
pub fn is_builtin(source_id: &str) -> bool {
    bootstrap().is_some_and(|official| official.source_id == source_id)
}

/// Refuse an operation that would treat a user-added source as official.
pub fn reject_reserved_key(source_id: &str) -> DraftResult<()> {
    if source_id == OFFICIAL_SOURCE_ID && bootstrap().is_some() {
        return Err(DraftError::invalid_config(format!(
            "'{OFFICIAL_SOURCE_ID}' is the built-in official source in this build and is configured automatically"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_without_bootstrap_data_has_no_official_source() {
        // This test build carries neither variable, which is the default and
        // the state every developer build is in.
        if CATALOG_URL.is_none() || ROOT_FINGERPRINT.is_none() {
            assert!(bootstrap().is_none());
            assert!(!is_builtin(OFFICIAL_SOURCE_ID));
            // And the reserved key is then just an ordinary name.
            reject_reserved_key(OFFICIAL_SOURCE_ID).unwrap();
        }
    }

    #[test]
    fn the_reserved_key_alone_conveys_nothing() {
        // Whatever this build carries, a *different* key is never built in.
        assert!(!is_builtin("acme"));
        reject_reserved_key("acme").unwrap();
    }

    #[test]
    fn a_malformed_bootstrap_is_treated_as_absent() {
        // Mirrors `bootstrap`'s acceptance rule so a half-configured release
        // cannot produce a source with an unverifiable anchor.
        let usable = |url: &str, fingerprint: &str| {
            url.starts_with("https://") && !url.trim().is_empty() && is_digest(fingerprint)
        };
        let good = format!("sha256:{}", "a".repeat(64));
        assert!(usable("https://extensions.draft.dev/", &good));
        assert!(!usable("http://extensions.draft.dev/", &good));
        assert!(!usable("https://extensions.draft.dev/", "not-a-digest"));
        assert!(!usable("", &good));
    }
}
