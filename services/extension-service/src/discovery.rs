//! Finding extensions, and deciding which one an install means.
//!
//! Search is deliberately forgiving and resolution is deliberately not. A
//! partial or misspelled query should still surface the right package, but
//! *installing* resolves an exact canonical extension id from an exact source:
//! Draft never installs the top-ranked guess, and never silently picks between
//! two sources publishing the same id.
//!
//! Everything here reads verified cached catalog metadata. Discovery works
//! offline by design; contacting a source happens only when the user asks for
//! a refresh.

use draft_core::extension::CatalogTarget;
use draft_core::support::error::{DraftError, DraftErrorKind, DraftResult};

use crate::catalog::{CatalogUsability, DiscoveredExtension};

/// Default page size for a search.
pub const DEFAULT_LIMIT: usize = 25;
/// Largest page a single search will return.
pub const MAX_LIMIT: usize = 200;

/// What to look for.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryQuery {
    /// Free text. Empty lists everything available.
    pub text: String,
    /// Restrict to one configured source.
    pub source_id: Option<String>,
    /// Restrict to packages declaring this capability.
    pub capability: Option<String>,
    /// 1-based page number.
    pub page: usize,
    pub limit: usize,
}

impl DiscoveryQuery {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            page: 1,
            limit: DEFAULT_LIMIT,
            ..Self::default()
        }
    }

    fn normalized(&self) -> (usize, usize) {
        (self.page.max(1), self.limit.clamp(1, MAX_LIMIT))
    }
}

/// One page of search results, with enough context to page through the rest.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscoveryResults {
    pub results: Vec<DiscoveredExtension>,
    /// Matches across every page, before paging.
    pub total: usize,
    pub page: usize,
    pub limit: usize,
    /// Sources that contributed nothing because they have never been
    /// refreshed, so an empty result is explainable rather than mysterious.
    pub unrefreshed_sources: Vec<String>,
}

/// Search verified cached catalog metadata.
pub fn search(query: &DiscoveryQuery) -> DraftResult<DiscoveryResults> {
    let (page, limit) = query.normalized();
    let needle = query.text.trim().to_ascii_lowercase();

    let mut scored: Vec<(u32, DiscoveredExtension)> = Vec::new();
    let mut unrefreshed = Vec::new();

    for candidate in crate::catalog::cached_targets()? {
        match candidate {
            CachedSource::Unrefreshed(source_id) => {
                if query
                    .source_id
                    .as_ref()
                    .is_none_or(|wanted| wanted == &source_id)
                {
                    unrefreshed.push(source_id);
                }
            }
            CachedSource::Targets(discovered) => {
                for entry in discovered {
                    if query
                        .source_id
                        .as_ref()
                        .is_some_and(|wanted| wanted != &entry.source_id)
                    {
                        continue;
                    }
                    if query
                        .capability
                        .as_ref()
                        .is_some_and(|capability| !entry.target.provides(capability))
                    {
                        continue;
                    }
                    let Some(score) = relevance(&entry.target, &needle) else {
                        continue;
                    };
                    scored.push((score, entry));
                }
            }
        }
    }

    scored.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.target.id.cmp(&right.1.target.id))
            .then_with(|| {
                crate::catalog::compare_versions(&right.1.target.version, &left.1.target.version)
            })
            .then_with(|| left.1.source_id.cmp(&right.1.source_id))
    });

    let total = scored.len();
    let results = scored
        .into_iter()
        .skip((page - 1) * limit)
        .take(limit)
        .map(|(_, entry)| entry)
        .collect();

    unrefreshed.sort();
    unrefreshed.dedup();
    Ok(DiscoveryResults {
        results,
        total,
        page,
        limit,
        unrefreshed_sources: unrefreshed,
    })
}

/// A source's contribution to a search.
pub(crate) enum CachedSource {
    /// The source has verified cached metadata.
    Targets(Vec<DiscoveredExtension>),
    /// The source is configured and enabled but has never been refreshed.
    Unrefreshed(String),
}

/// How well `target` matches `needle`.
///
/// `None` means no match at all. An empty needle matches everything equally,
/// so listing falls out of the same path as searching.
fn relevance(target: &CatalogTarget, needle: &str) -> Option<u32> {
    if needle.is_empty() {
        return Some(1);
    }
    let id = target.id.to_ascii_lowercase();
    let mut score = 0;
    if id == needle {
        score += 1000;
    } else if id.starts_with(needle) {
        score += 500;
    } else if id.contains(needle) {
        score += 300;
    }
    if let Some(name) = &target.name {
        if name.to_ascii_lowercase().contains(needle) {
            score += 200;
        }
    }
    if target
        .keywords
        .iter()
        .any(|keyword| keyword.eq_ignore_ascii_case(needle))
        || target
            .capabilities
            .iter()
            .any(|capability| capability.eq_ignore_ascii_case(needle))
    {
        score += 150;
    }
    if target.publisher.to_ascii_lowercase().contains(needle) {
        score += 100;
    }
    if let Some(description) = &target.description {
        if description.to_ascii_lowercase().contains(needle) {
            score += 50;
        }
    }
    // Fall back to any signed field containing the needle, so a keyword-only
    // hit still surfaces even when nothing above matched.
    if score == 0
        && target
            .searchable_text()
            .iter()
            .any(|field| field.contains(needle))
    {
        score += 10;
    }
    (score > 0).then_some(score)
}

/// Where an install of `package_id` should come from.
///
/// With `--source` this only checks the source really publishes it. Without
/// one, it resolves only when exactly one eligible source does — otherwise it
/// refuses and names the candidates rather than choosing for the user.
pub fn resolve_source(package_id: &str, explicit: Option<&str>) -> DraftResult<String> {
    let publishing = publishing_sources(package_id)?;

    if let Some(source_id) = explicit {
        if !publishing.iter().any(|candidate| candidate == source_id) {
            return Err(DraftError::not_found(format!(
                "extension source '{source_id}' does not publish '{package_id}'"
            )));
        }
        return Ok(source_id.to_string());
    }

    match publishing.as_slice() {
        [] => Err(DraftError::not_found(format!(
            "no configured extension source publishes '{package_id}'"
        ))),
        [only] => Ok(only.clone()),
        several => Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            format!(
                "extension '{package_id}' is available from multiple sources:\n  {}\nSpecify:\n  --source <source-key>",
                several.join("\n  ")
            ),
        )),
    }
}

/// Every enabled source whose verified cache publishes `package_id`.
fn publishing_sources(package_id: &str) -> DraftResult<Vec<String>> {
    let mut sources: Vec<String> = Vec::new();
    for candidate in crate::catalog::cached_targets()? {
        if let CachedSource::Targets(discovered) = candidate {
            for entry in discovered {
                if entry.target.id == package_id && !sources.contains(&entry.source_id) {
                    sources.push(entry.source_id);
                }
            }
        }
    }
    sources.sort();
    Ok(sources)
}

/// The source an installed extension must be updated from.
///
/// Update follows the lineage recorded at install time. Another source
/// publishing the same id is a different package as far as Draft is concerned,
/// and switching between them is an explicit uninstall and reinstall.
pub fn update_source(package_id: &str, explicit: Option<&str>) -> DraftResult<String> {
    let installed = crate::extension::show(package_id)?;
    let recorded = installed.update_source_id().ok_or_else(|| {
        DraftError::invalid_config(format!(
            "extension '{package_id}' was installed from a local directory, so it has no source to update from"
        ))
    })?;
    if let Some(source_id) = explicit {
        if source_id != recorded {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!(
                    "extension '{package_id}' was installed from source '{recorded}'; \
                     updating from '{source_id}' would change its provenance. \
                     Uninstall and reinstall to change source."
                ),
            ));
        }
    }
    Ok(recorded.to_string())
}

/// Whether `freshness` permits authorizing an installation.
pub fn permits_install(freshness: &CatalogUsability) -> bool {
    matches!(freshness, CatalogUsability::Usable)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(
        id: &str,
        name: Option<&str>,
        keywords: &[&str],
        capabilities: &[&str],
    ) -> CatalogTarget {
        CatalogTarget {
            id: id.into(),
            version: "1.0.0".into(),
            publisher: "draft".into(),
            draft_api: "^0.3.4".into(),
            artifact_path: format!("artifacts/{id}.tar"),
            length: 1,
            sha256: format!("sha256:{}", "0".repeat(64)),
            name: name.map(ToString::to_string),
            description: Some("Language support for the example toolchain".into()),
            keywords: keywords.iter().map(|value| (*value).to_string()).collect(),
            capabilities: capabilities
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
        }
    }

    #[test]
    fn an_exact_id_outranks_a_partial_one() {
        let exact = target("draft.language.rust", Some("Rust"), &[], &[]);
        let partial = target("draft.language.rustfmt", Some("Rustfmt"), &[], &[]);
        assert!(
            relevance(&exact, "draft.language.rust").unwrap()
                > relevance(&partial, "draft.language.rust").unwrap()
        );
    }

    #[test]
    fn search_matches_names_keywords_and_capabilities() {
        let package = target(
            "draft.language.example",
            Some("Example language"),
            &["ecs"],
            &["verification"],
        );
        assert!(relevance(&package, "example").is_some());
        assert!(relevance(&package, "ecs").is_some());
        assert!(relevance(&package, "verification").is_some());
        assert!(relevance(&package, "toolchain").is_some(), "description");
        assert!(relevance(&package, "nothing-like-this").is_none());
    }

    #[test]
    fn an_empty_query_lists_everything() {
        let package = target("draft.language.example", None, &[], &[]);
        assert_eq!(relevance(&package, ""), Some(1));
    }

    #[test]
    fn capability_filtering_is_case_insensitive() {
        let package = target("draft.language.example", None, &[], &["Verification"]);
        assert!(package.provides("verification"));
        assert!(!package.provides("symbol_extraction"));
    }

    #[test]
    fn paging_is_clamped_to_sane_bounds() {
        let query = DiscoveryQuery {
            page: 0,
            limit: 0,
            ..DiscoveryQuery::new("")
        };
        assert_eq!(query.normalized(), (1, 1));

        let query = DiscoveryQuery {
            page: 3,
            limit: MAX_LIMIT * 10,
            ..DiscoveryQuery::new("")
        };
        assert_eq!(query.normalized(), (3, MAX_LIMIT));
    }

    #[test]
    fn only_a_usable_catalog_authorizes_an_install() {
        assert!(permits_install(&CatalogUsability::Usable));
        for refused in [
            CatalogUsability::Untrusted,
            CatalogUsability::Expired,
            CatalogUsability::Invalid,
            CatalogUsability::Unavailable,
        ] {
            assert!(!permits_install(&refused));
        }
    }
}
