//! Matching a resource against a predicate, over intrinsic facts alone.
//!
//! # Why this is not in `extension`
//!
//! It reads like extension machinery because contributed rules use it, but
//! nothing here is about extensions. It is a small total function over facts a
//! resource already has — its scheme, body, media type, form, attributes and
//! size — and it evaluates the same way whoever wrote the predicate.
//!
//! `project` needs it to decide what is protected, and a project's own
//! protections cannot depend on what happens to be installed. Putting the
//! evaluator below both layers is what makes that true structurally rather
//! than by convention.
//!
//! The predicate *vocabulary* is the SDK's; this is only its evaluation.

use crate::support::glob;
use draft_extension_contract::{
    AttributeMatch, AttributeValue, RawResourcePredicate, ResourceForm,
};
use std::collections::BTreeMap;

/// The intrinsic facts a predicate may test.
///
/// This is a borrowed view rather than a workspace type on purpose: the
/// extension port depends only on `contracts` and `support`, so the layer that
/// owns resource state builds this view and hands it down. Nothing here is
/// derived — a class assignment is passed separately, because classification is
/// interpretation and must not masquerade as observed state.
#[derive(Debug, Clone, Copy)]
pub struct ResourceView<'a> {
    pub locator_scheme: &'a str,
    /// Opaque to Core. Matched as a plain string; never parsed for structure.
    pub locator_body: &'a str,
    pub media_type: Option<&'a str>,
    pub form: Option<ResourceForm>,
    pub attributes: &'a BTreeMap<String, AttributeValue>,
    pub content_size: Option<u64>,
}

impl ResourceView<'_> {
    /// Whether this resource is addressed by a filesystem locator.
    ///
    /// The path predicates are meaningful only here. For any other scheme they
    /// do not match, rather than matching a body that merely looks path-like.
    fn is_filesystem(&self) -> bool {
        self.locator_scheme == FILE_SCHEME
    }
}

/// The locator scheme Draft's built-in adapter owns.
pub const FILE_SCHEME: &str = "file";

/// Evaluate a predicate over intrinsic facts alone.
pub fn matches_raw(predicate: &RawResourcePredicate, resource: &ResourceView<'_>) -> bool {
    match predicate {
        RawResourcePredicate::All { of } => of.iter().all(|p| matches_raw(p, resource)),
        RawResourcePredicate::Any { of } => of.iter().any(|p| matches_raw(p, resource)),
        RawResourcePredicate::Not { of } => !matches_raw(of, resource),
        RawResourcePredicate::LocatorScheme { equals } => resource.locator_scheme == equals,
        RawResourcePredicate::LocatorPattern { glob } => glob::matches(glob, resource.locator_body),
        RawResourcePredicate::MediaType { equals } => resource.media_type == Some(equals.as_str()),
        RawResourcePredicate::Form { equals } => resource.form == Some(*equals),
        RawResourcePredicate::Attribute { name, matches } => resource
            .attributes
            .get(name)
            .is_some_and(|value| attribute_matches(matches, value)),
        RawResourcePredicate::ContentSize { at_least, at_most } => {
            let Some(size) = resource.content_size else {
                return false;
            };
            at_least.is_none_or(|bound| size >= bound) && at_most.is_none_or(|bound| size <= bound)
        }
        // Filesystem-only predicates. A non-file locator never matches, so a
        // rule written for files cannot silently capture a catalog or timeline
        // resource whose body happens to contain slashes.
        RawResourcePredicate::PathGlob { glob } => {
            resource.is_filesystem() && glob::matches(glob, resource.locator_body)
        }
        RawResourcePredicate::PathSuffix { suffix } => {
            resource.is_filesystem() && resource.locator_body.ends_with(suffix.as_str())
        }
    }
}

fn attribute_matches(rule: &AttributeMatch, value: &AttributeValue) -> bool {
    match (rule, value) {
        (AttributeMatch::Equals { value: expected }, actual) => expected == actual,
        (AttributeMatch::Prefix { value: prefix }, AttributeValue::Text(text)) => {
            text.starts_with(prefix)
        }
        (AttributeMatch::Suffix { value: suffix }, AttributeValue::Text(text)) => {
            text.ends_with(suffix)
        }
        (AttributeMatch::Glob { pattern }, AttributeValue::Text(text)) => {
            glob::matches(pattern, text)
        }
        (AttributeMatch::Range { at_least, at_most }, AttributeValue::Integer(number)) => {
            at_least.is_none_or(|bound| *number >= bound)
                && at_most.is_none_or(|bound| *number <= bound)
        }
        // A string rule against a non-string value is simply not a match; it is
        // never coerced, so a numeric attribute cannot be caught by a prefix.
        _ => false,
    }
}
