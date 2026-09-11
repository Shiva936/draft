//! Sequence alignment over an opaque token stream.
//!
//! This engine knows three things: how to cut a byte window into tokens
//! according to a contributed rule, how to align two token streams, and how to
//! name the result in a coordinate space the contributor chose. It does not
//! know what a line is, that text exists, or that the space it is emitting
//! coordinates in has anything to do with a file.
//!
//! An extension that configures `delimited{delimiter_bytes: [0x0A]}` and calls
//! the space `draft.text.document/line` has produced line-level review units —
//! but that meaning lives entirely in the extension.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{EngineOutput, EngineUnit};
use crate::dcg::representation::{ConflictClaim, ConflictScope};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

pub const REVISION: u32 = 1;

/// The largest window this engine will align.
///
/// Beyond it the resource is not aligned at all, and the caller falls back to a
/// whole-resource claim: a truncated alignment would silently under-report
/// where a change landed.
pub const DEFAULT_BYTE_BUDGET: u64 = 4 * 1024 * 1024;

/// How to cut a byte window into tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Tokenizer {
    /// Split on a byte sequence.
    Delimited {
        /// The delimiter, as bytes. Never assumed to be text.
        delimiter_bytes: Vec<u8>,
        /// Whether the delimiter stays attached to the token before it.
        #[serde(default)]
        include_delimiter: bool,
    },
    /// Split into fixed-width chunks — for a framed or record-oriented stream.
    FixedWidth { width: u64 },
}

/// The contributed configuration this engine runs under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlignmentConfig {
    pub tokenizer: Tokenizer,
    /// The name of the coordinate space the emitted regions live in. Opaque to
    /// Core, which only ever compares it for equality with another one.
    pub coordinate_space: String,
    #[serde(default = "default_budget")]
    pub byte_budget: u64,
}

fn default_budget() -> u64 {
    DEFAULT_BYTE_BUDGET
}

impl AlignmentConfig {
    pub fn parse(config: &serde_json::Value) -> DraftResult<Self> {
        serde_json::from_value(config.clone()).map_err(|error| {
            DraftError::invalid_config(format!("invalid sequence_alignment config: {error}"))
        })
    }

    fn tokenize<'a>(&self, bytes: &'a [u8]) -> Vec<&'a [u8]> {
        match &self.tokenizer {
            Tokenizer::Delimited {
                delimiter_bytes,
                include_delimiter,
            } => split_on(bytes, delimiter_bytes, *include_delimiter),
            Tokenizer::FixedWidth { width } => {
                let width = (*width).max(1) as usize;
                bytes.chunks(width).collect()
            }
        }
    }
}

/// Align two byte windows and claim the regions that differ.
pub fn compare(
    config: &AlignmentConfig,
    before: Option<&[u8]>,
    after: Option<&[u8]>,
) -> DraftResult<EngineOutput> {
    let before = before.unwrap_or(&[]);
    let after = after.unwrap_or(&[]);
    let total = before.len() as u64 + after.len() as u64;
    if total > config.byte_budget {
        return Err(DraftError::new(
            DraftErrorKind::Validation,
            format!(
                "sequence alignment window of {total} bytes exceeds the declared budget of {}",
                config.byte_budget
            ),
        ));
    }
    if config.coordinate_space.trim().is_empty() {
        return Err(DraftError::invalid_config(
            "sequence_alignment requires a contributed coordinate_space",
        ));
    }

    let before_tokens = config.tokenize(before);
    let after_tokens = config.tokenize(after);
    let regions = align(&before_tokens, &after_tokens);

    let mut units = Vec::with_capacity(regions.len());
    let mut claims = Vec::with_capacity(regions.len());
    let mut added = 0i64;
    let mut removed = 0i64;
    for (index, region) in regions.iter().enumerate() {
        added += region.after_length as i64;
        removed += region.before_length as i64;
        let unit_id = format!("{}:{}", config.coordinate_space, index);
        // The claim is made in the *result* coordinate space, at the position
        // the change occupies after it is applied. An insertion has zero length
        // there, which is what keeps two insertions at the same position
        // composable rather than falsely conflicting.
        let scope = ConflictScope::LinearRegion {
            coordinate_space: config.coordinate_space.clone(),
            start: region.after_start,
            length: region.after_length,
        };
        units.push(EngineUnit {
            unit_id: unit_id.clone(),
            scope: scope.clone(),
            label: format!(
                "{} {}..{}",
                config.coordinate_space,
                region.after_start,
                region.after_start + region.after_length
            ),
        });
        claims.push(ConflictClaim { id: unit_id, scope });
    }

    let mut metrics = BTreeMap::new();
    metrics.insert("draft.core/units_added".to_string(), added);
    metrics.insert("draft.core/units_removed".to_string(), removed);
    metrics.insert(
        "draft.core/regions_changed".to_string(),
        regions.len() as i64,
    );
    Ok(EngineOutput {
        units,
        claims,
        metrics,
    })
}

/// One contiguous divergence between the two token streams.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Region {
    before_start: u64,
    before_length: u64,
    after_start: u64,
    after_length: u64,
}

/// Align two token streams and return the regions that differ.
///
/// A longest-common-subsequence alignment, computed over token equality alone.
/// Tokens are opaque byte slices: this never inspects their contents beyond
/// comparing them.
fn align(before: &[&[u8]], after: &[&[u8]]) -> Vec<Region> {
    let common = longest_common_subsequence(before, after);

    let mut regions = Vec::new();
    let mut before_index = 0usize;
    let mut after_index = 0usize;
    let mut pending: Option<Region> = None;

    let flush = |pending: &mut Option<Region>, regions: &mut Vec<Region>| {
        if let Some(region) = pending.take() {
            regions.push(region);
        }
    };

    for (matched_before, matched_after) in common
        .iter()
        .copied()
        .chain(std::iter::once((before.len(), after.len())))
    {
        let before_run = matched_before - before_index;
        let after_run = matched_after - after_index;
        if before_run > 0 || after_run > 0 {
            let region = Region {
                before_start: before_index as u64,
                before_length: before_run as u64,
                after_start: after_index as u64,
                after_length: after_run as u64,
            };
            match &mut pending {
                Some(existing) => {
                    existing.before_length += region.before_length;
                    existing.after_length += region.after_length;
                }
                None => pending = Some(region),
            }
        }
        flush(&mut pending, &mut regions);
        before_index = matched_before.saturating_add(1).min(before.len());
        after_index = matched_after.saturating_add(1).min(after.len());
        if matched_before == before.len() {
            break;
        }
    }
    flush(&mut pending, &mut regions);
    regions
}

/// Indices of a longest common subsequence, as `(before_index, after_index)`.
fn longest_common_subsequence(before: &[&[u8]], after: &[&[u8]]) -> Vec<(usize, usize)> {
    let rows = before.len();
    let columns = after.len();
    if rows == 0 || columns == 0 {
        return Vec::new();
    }
    let mut table = vec![0u32; (rows + 1) * (columns + 1)];
    let at = |row: usize, column: usize| row * (columns + 1) + column;
    for row in (0..rows).rev() {
        for column in (0..columns).rev() {
            table[at(row, column)] = if before[row] == after[column] {
                table[at(row + 1, column + 1)] + 1
            } else {
                table[at(row + 1, column)].max(table[at(row, column + 1)])
            };
        }
    }
    let mut matches = Vec::new();
    let (mut row, mut column) = (0usize, 0usize);
    while row < rows && column < columns {
        if before[row] == after[column] {
            matches.push((row, column));
            row += 1;
            column += 1;
        } else if table[at(row + 1, column)] >= table[at(row, column + 1)] {
            row += 1;
        } else {
            column += 1;
        }
    }
    matches
}

/// Split a byte slice on a delimiter.
fn split_on<'a>(bytes: &'a [u8], delimiter: &[u8], include: bool) -> Vec<&'a [u8]> {
    if delimiter.is_empty() || bytes.is_empty() {
        return if bytes.is_empty() {
            Vec::new()
        } else {
            vec![bytes]
        };
    }
    let mut tokens = Vec::new();
    let mut start = 0usize;
    let mut cursor = 0usize;
    while cursor + delimiter.len() <= bytes.len() {
        if &bytes[cursor..cursor + delimiter.len()] == delimiter {
            let end = if include {
                cursor + delimiter.len()
            } else {
                cursor
            };
            tokens.push(&bytes[start..end]);
            cursor += delimiter.len();
            start = cursor;
        } else {
            cursor += 1;
        }
    }
    if start < bytes.len() {
        tokens.push(&bytes[start..]);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(space: &str) -> AlignmentConfig {
        AlignmentConfig {
            tokenizer: Tokenizer::Delimited {
                delimiter_bytes: vec![b'\n'],
                include_delimiter: false,
            },
            coordinate_space: space.to_string(),
            byte_budget: DEFAULT_BYTE_BUDGET,
        }
    }

    #[test]
    fn it_claims_only_the_regions_that_changed() {
        let config = lines("draft.text.document/line");
        let output = compare(
            &config,
            Some(b"alpha\nbeta\ngamma"),
            Some(b"alpha\nBETA\ngamma"),
        )
        .unwrap();
        assert_eq!(output.claims.len(), 1, "{:?}", output.claims);
        assert_eq!(
            output.claims[0].scope,
            ConflictScope::LinearRegion {
                coordinate_space: "draft.text.document/line".into(),
                start: 1,
                length: 1,
            }
        );
    }

    #[test]
    fn changes_in_different_regions_stay_composable() {
        let config = lines("draft.text.document/line");
        let first = compare(&config, Some(b"a\nb\nc\nd\ne"), Some(b"A\nb\nc\nd\ne")).unwrap();
        let second = compare(&config, Some(b"a\nb\nc\nd\ne"), Some(b"a\nb\nc\nd\nE")).unwrap();
        assert_eq!(
            crate::dcg::representation::reconcile(&first.claims, &second.claims),
            crate::dcg::representation::ClaimRelation::Independent
        );
    }

    #[test]
    fn the_same_region_conflicts() {
        let config = lines("draft.text.document/line");
        let first = compare(&config, Some(b"a\nb\nc"), Some(b"a\nX\nc")).unwrap();
        let second = compare(&config, Some(b"a\nb\nc"), Some(b"a\nY\nc")).unwrap();
        assert!(
            !crate::dcg::representation::reconcile(&first.claims, &second.claims).is_composable()
        );
    }

    #[test]
    fn a_different_coordinate_space_is_indeterminate_not_independent() {
        let text = compare(
            &lines("draft.text.document/line"),
            Some(b"a\nb"),
            Some(b"a\nX"),
        )
        .unwrap();
        let frames = compare(&lines("example/frame"), Some(b"a\nb"), Some(b"a\nX")).unwrap();
        // Two coordinate systems Core cannot relate. It says so rather than
        // guessing, and fails closed.
        assert!(matches!(
            crate::dcg::representation::reconcile(&text.claims, &frames.claims),
            crate::dcg::representation::ClaimRelation::Indeterminate { .. }
        ));
    }

    #[test]
    fn the_tokenizer_is_bytes_not_text() {
        // A framed binary stream: no newlines, no text, still aligned.
        let config = AlignmentConfig {
            tokenizer: Tokenizer::FixedWidth { width: 4 },
            coordinate_space: "example/frame".into(),
            byte_budget: DEFAULT_BYTE_BUDGET,
        };
        let output = compare(
            &config,
            Some(&[0, 1, 2, 3, 4, 5, 6, 7]),
            Some(&[0, 1, 2, 3, 9, 9, 9, 9]),
        )
        .unwrap();
        assert_eq!(output.claims.len(), 1);
        assert_eq!(
            output.claims[0].scope,
            ConflictScope::LinearRegion {
                coordinate_space: "example/frame".into(),
                start: 1,
                length: 1,
            }
        );
    }

    #[test]
    fn an_oversized_window_is_refused_rather_than_truncated() {
        let mut config = lines("draft.text.document/line");
        config.byte_budget = 8;
        let error = compare(&config, Some(b"aaaaaaaaaa"), Some(b"bbbbbbbbbb")).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::Validation);
    }

    #[test]
    fn an_insertion_claims_zero_length_at_its_position() {
        let config = lines("draft.text.document/line");
        let output = compare(&config, Some(b"a\nc"), Some(b"a\nb\nc")).unwrap();
        assert_eq!(
            output.claims[0].scope,
            ConflictScope::LinearRegion {
                coordinate_space: "draft.text.document/line".into(),
                start: 1,
                length: 1,
            }
        );
    }
}
