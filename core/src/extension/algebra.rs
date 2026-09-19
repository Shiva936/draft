//! How contributions from several extensions combine.
//!
//! Draft never resolves by iteration order, and it never picks a winner it was
//! not asked to pick. But "never pick a winner" is not one rule — it means
//! different things for different kinds of contribution, and conflating them is
//! how a platform ends up either refusing a legitimate combination or silently
//! preferring one publisher over another.
//!
//! Four algebras cover every contribution kind Draft consumes:
//!
//! * **Unique resolution** — at most one answer may apply, so disagreement is
//!   [`Resolution::Ambiguous`] and Draft acts on none of it. One adapter per
//!   locator scheme; one comparison strategy per resource.
//! * **Keyed union** — independently named things coexist. Two extensions
//!   contributing different check ids both run; two contributing the *same* id
//!   are ambiguous for that id alone, and every other id is unaffected.
//! * **Aggregation** — everything applies and every contributor is cited, as
//!   for risk rules.
//! * **Conservative merge** — policy halves combine by taking the most
//!   restrictive value.
//!
//! Which algebra a kind uses is data, not a convention scattered through call
//! sites: [`composition_of`] states it once, and a test enumerates it.

use super::capability::{Contributed, Resolution};
use draft_extension_contract::ExtensionContributionKind;
use std::collections::BTreeMap;

/// How the contributions of one kind combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Composition {
    /// At most one may apply; disagreement is ambiguous and nothing is chosen.
    UniqueResolution,
    /// Independently named entries coexist; only a repeated key is ambiguous.
    KeyedUnion,
    /// Every entry applies, and every contributor is recorded.
    Aggregate,
    /// Entries merge by taking the most restrictive value.
    ConservativeMerge,
    /// Selected by declared specificity; an exact tie is ambiguous.
    SpecificitySelection,
}

/// The algebra each contribution kind composes under.
///
/// Every kind has exactly one, and the mapping is total: adding a kind without
/// deciding how it composes is a compile error, not a runtime surprise.
pub const fn composition_of(kind: ExtensionContributionKind) -> Composition {
    use ExtensionContributionKind as Kind;
    match kind {
        Kind::ResourceAdapter => Composition::UniqueResolution,
        Kind::ResourceClassification => Composition::KeyedUnion,
        Kind::Comparison => Composition::UniqueResolution,
        Kind::ElementExtraction => Composition::KeyedUnion,
        Kind::Presentation => Composition::SpecificitySelection,
        Kind::ToolAction => Composition::KeyedUnion,
        Kind::Verification => Composition::KeyedUnion,
        Kind::RiskRule => Composition::Aggregate,
        Kind::PolicyPreset => Composition::ConservativeMerge,
        Kind::IntentVocabulary => Composition::KeyedUnion,
        Kind::TaskTemplate => Composition::KeyedUnion,
        Kind::CandidatePreset => Composition::KeyedUnion,
        Kind::Documentation => Composition::Aggregate,
    }
}

/// Coalesce every applicable contribution into a single outcome.
///
/// `equivalent` decides whether two contributions mean the same thing, and it is
/// deliberately per contribution kind: two classifications agree when they name
/// the same class, but two comparison strategies that share a subject may still
/// produce different representations, so they do not.
///
/// Candidates are sorted by extension id purely so diagnostics and tests are
/// reproducible. The order never selects a winner: a set with more than one
/// meaning is `Ambiguous` whatever order it arrived in.
pub fn resolve_unique<'a, T>(
    mut applicable: Vec<&'a Contributed<T>>,
    equivalent: impl Fn(&T, &T) -> bool,
) -> Resolution<'a, T> {
    applicable.sort_by(|a, b| a.extension_id.cmp(&b.extension_id));
    let Some(first) = applicable.first().copied() else {
        return Resolution::NoMatch;
    };
    if applicable
        .iter()
        .all(|candidate| equivalent(&candidate.value, &first.value))
    {
        return Resolution::Resolved {
            value: &first.value,
            contributors: applicable
                .iter()
                .map(|candidate| candidate.extension_id.as_str())
                .collect(),
        };
    }
    Resolution::Ambiguous {
        candidates: applicable,
    }
}

/// Select the most specific applicable contribution.
///
/// `specificity` is declared by the contribution's own binding, never inferred
/// and never a number a publisher can inflate to capture the primary human
/// view. Only the most specific tier competes; anything less specific is not a
/// tie-breaker but simply not selected.
///
/// An exact tie within that tier is [`Resolution::Ambiguous`] unless every
/// candidate means the same thing. Draft does not arbitrate between two
/// publishers who both claim to be the authoritative way to show something —
/// the caller offers the choice, or falls back to the neutral rendering that
/// always exists.
pub fn resolve_by_specificity<'a, T>(
    applicable: Vec<&'a Contributed<T>>,
    specificity: impl Fn(&T) -> u8,
    equivalent: impl Fn(&T, &T) -> bool,
) -> Resolution<'a, T> {
    let Some(most) = applicable
        .iter()
        .map(|candidate| specificity(&candidate.value))
        .max()
    else {
        return Resolution::NoMatch;
    };
    resolve_unique(
        applicable
            .into_iter()
            .filter(|candidate| specificity(&candidate.value) == most)
            .collect(),
        equivalent,
    )
}

/// One entry of a keyed union: either the single agreed contribution for a key,
/// or a scoped collision naming every publisher that claimed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyedEntry<'a, T> {
    Resolved {
        value: &'a T,
        contributors: Vec<&'a str>,
    },
    /// Several extensions claimed the same key with different meanings. Only
    /// this key is unusable; every other key in the union is unaffected.
    Collision { candidates: Vec<&'a Contributed<T>> },
}

impl<'a, T> KeyedEntry<'a, T> {
    pub fn value(&self) -> Option<&'a T> {
        match self {
            Self::Resolved { value, .. } => Some(value),
            Self::Collision { .. } => None,
        }
    }

    pub fn contributors(&self) -> Vec<&'a str> {
        match self {
            Self::Resolved { contributors, .. } => contributors.clone(),
            Self::Collision { candidates } => candidates
                .iter()
                .map(|candidate| candidate.extension_id.as_str())
                .collect(),
        }
    }
}

/// Union contributions by a namespaced key.
///
/// This is what lets two extensions cover the same subject without either being
/// discarded: a text classifier and a language classifier both apply to the same
/// resource, under different keys, and both survive. A repeated key with
/// differing meanings collides — but the collision is scoped to that key, so the
/// rest of the union keeps working.
pub fn union_by_key<'a, T, K>(
    contributions: impl IntoIterator<Item = &'a Contributed<T>>,
    key_of: impl Fn(&'a T) -> K,
    equivalent: impl Fn(&T, &T) -> bool,
) -> BTreeMap<K, KeyedEntry<'a, T>>
where
    T: 'a,
    K: Ord,
{
    let mut grouped: BTreeMap<K, Vec<&'a Contributed<T>>> = BTreeMap::new();
    for contribution in contributions {
        grouped
            .entry(key_of(&contribution.value))
            .or_default()
            .push(contribution);
    }
    grouped
        .into_iter()
        .map(|(key, mut candidates)| {
            candidates.sort_by(|a, b| a.extension_id.cmp(&b.extension_id));
            let first = candidates[0];
            let entry = if candidates
                .iter()
                .all(|candidate| equivalent(&candidate.value, &first.value))
            {
                KeyedEntry::Resolved {
                    value: &first.value,
                    contributors: candidates
                        .iter()
                        .map(|candidate| candidate.extension_id.as_str())
                        .collect(),
                }
            } else {
                KeyedEntry::Collision { candidates }
            };
            (key, entry)
        })
        .collect()
}

/// Aggregate every contribution, retaining its contributor.
///
/// Nothing is dropped and nothing is deduplicated by value: two extensions
/// contributing the same risk rule both applied it, and a report that named only
/// one of them would misattribute the outcome.
pub fn aggregate<'a, T, I>(
    contributions: impl IntoIterator<Item = &'a Contributed<Vec<T>>>,
    mut each: impl FnMut(&'a str, &'a T) -> I,
) -> Vec<I>
where
    T: 'a,
{
    let mut sources: Vec<&Contributed<Vec<T>>> = contributions.into_iter().collect();
    sources.sort_by(|a, b| a.extension_id.cmp(&b.extension_id));
    sources
        .into_iter()
        .flat_map(|source| {
            source
                .value
                .iter()
                .map(|item| each(source.extension_id.as_str(), item))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Take the more restrictive of two optional bounds.
///
/// `None` means "no opinion", so it never loosens a bound another contributor
/// set. This is what makes policy merging safe regardless of install order.
pub fn most_restrictive_max<T: Ord + Copy>(left: Option<T>, right: Option<T>) -> Option<T> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

/// Take the more restrictive of two thresholds, where lower means stricter.
pub fn most_restrictive_threshold(left: Option<u32>, right: Option<u32>) -> Option<u32> {
    most_restrictive_max(left, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contributed(id: &str, value: &str) -> Contributed<String> {
        Contributed::new(id, value.to_string())
    }

    #[test]
    fn every_contribution_kind_declares_exactly_one_algebra() {
        // The mapping is total by construction; this asserts the intent behind
        // each entry so a future kind cannot be added without a decision.
        use ExtensionContributionKind as Kind;
        let expected = [
            (Kind::ResourceAdapter, Composition::UniqueResolution),
            (Kind::ResourceClassification, Composition::KeyedUnion),
            (Kind::Comparison, Composition::UniqueResolution),
            (Kind::ElementExtraction, Composition::KeyedUnion),
            (Kind::Presentation, Composition::SpecificitySelection),
            (Kind::ToolAction, Composition::KeyedUnion),
            (Kind::Verification, Composition::KeyedUnion),
            (Kind::RiskRule, Composition::Aggregate),
            (Kind::PolicyPreset, Composition::ConservativeMerge),
            (Kind::IntentVocabulary, Composition::KeyedUnion),
            (Kind::TaskTemplate, Composition::KeyedUnion),
            (Kind::CandidatePreset, Composition::KeyedUnion),
            (Kind::Documentation, Composition::Aggregate),
        ];
        assert_eq!(expected.len(), ExtensionContributionKind::ALL.len());
        for (kind, composition) in expected {
            assert_eq!(composition_of(kind), composition, "{kind}");
        }
    }

    #[test]
    fn unique_resolution_never_selects_a_winner() {
        let alpha = contributed("ex.alpha", "one");
        let zed = contributed("ex.zed", "other");
        let forwards = resolve_unique(vec![&alpha, &zed], |a, b| a == b);
        let backwards = resolve_unique(vec![&zed, &alpha], |a, b| a == b);
        assert!(forwards.is_ambiguous());
        assert_eq!(forwards.value(), None);
        // Order in cannot change the outcome, and the report names everyone.
        assert_eq!(forwards.contributors(), backwards.contributors());
        assert_eq!(forwards.contributors(), vec!["ex.alpha", "ex.zed"]);
    }

    #[test]
    fn agreement_coalesces_and_keeps_every_contributor() {
        let alpha = contributed("ex.alpha", "same");
        let zed = contributed("ex.zed", "same");
        match resolve_unique(vec![&zed, &alpha], |a, b| a == b) {
            Resolution::Resolved {
                value,
                contributors,
            } => {
                assert_eq!(value, "same");
                assert_eq!(contributors, vec!["ex.alpha", "ex.zed"]);
            }
            other => panic!("agreement must resolve, got {other:?}"),
        }
    }

    #[test]
    fn a_keyed_union_lets_different_keys_coexist() {
        // The case that matters: two publishers legitimately describing the same
        // subject under different names. Neither is discarded, and neither makes
        // the other ambiguous.
        let text = contributed("draft.text", "document");
        let rust = contributed("draft.language.rust", "source");
        let union = union_by_key(vec![&text, &rust], |value| value.clone(), |a, b| a == b);
        assert_eq!(union.len(), 2);
        assert_eq!(union["document"].value(), Some(&"document".to_string()));
        assert_eq!(union["source"].value(), Some(&"source".to_string()));
    }

    #[test]
    fn a_repeated_key_collides_only_for_that_key() {
        let shared_a = Contributed::new("ex.alpha", ("shared", "meaning-a"));
        let shared_b = Contributed::new("ex.zed", ("shared", "meaning-b"));
        let other = Contributed::new("ex.zed", ("other", "fine"));
        let union = union_by_key(
            vec![&shared_a, &shared_b, &other],
            |value| value.0,
            |a, b| a == b,
        );
        assert!(matches!(union["shared"], KeyedEntry::Collision { .. }));
        assert_eq!(
            union["shared"].contributors(),
            vec!["ex.alpha", "ex.zed"],
            "a collision must name every claimant"
        );
        // The unrelated key is untouched: one publisher's mistake does not
        // disable the rest of the union.
        assert_eq!(union["other"].value(), Some(&("other", "fine")));
    }

    #[test]
    fn aggregation_retains_duplicates_and_attributes_each() {
        let alpha = Contributed::new("ex.alpha", vec!["rule".to_string()]);
        let zed = Contributed::new("ex.zed", vec!["rule".to_string()]);
        let applied = aggregate(vec![&zed, &alpha], |id, rule| (id, rule.clone()));
        // Both applied it; a report crediting only one would misattribute the
        // outcome.
        assert_eq!(
            applied,
            vec![
                ("ex.alpha", "rule".to_string()),
                ("ex.zed", "rule".to_string())
            ]
        );
    }

    #[test]
    fn conservative_merge_never_loosens_a_bound() {
        assert_eq!(most_restrictive_max(Some(10u32), Some(4)), Some(4));
        assert_eq!(most_restrictive_max(Some(10u32), None), Some(10));
        assert_eq!(most_restrictive_max(None, Some(4u32)), Some(4));
        assert_eq!(most_restrictive_max::<u32>(None, None), None);
        // Order independent, so install order cannot change the effective policy.
        assert_eq!(
            most_restrictive_threshold(Some(3), Some(9)),
            most_restrictive_threshold(Some(9), Some(3))
        );
    }
}
