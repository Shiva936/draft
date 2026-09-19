//! The derived explanation of a change, and the neutral algebra over it.
//!
//! A change set says *what* changed. A representation says *how*, in whatever
//! terms the owning domain uses. The split matters: a representation is optional
//! and regenerable, and installing, updating or removing the extension that
//! produces one must never alter the authoritative transition it explains.
//!
//! Core never parses a representation payload. It validates that the payload is
//! canonical, bounded and matches its declared schema, stores it, and reads only
//! two things from it — the summary metrics a risk rule may reference by key,
//! and the conflict claims and review units, both of which are structured
//! precisely so Core can act on them without knowing what they mean.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Largest inline representation payload Core will hold in a record.
///
/// Anything larger is stored as an object and referenced, so a pathological
/// representation cannot make a ChangePack record unreadable.
pub const MAX_INLINE_PAYLOAD_BYTES: usize = 256 * 1024;

/// Where a representation's payload lives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "payload", rename_all = "snake_case", deny_unknown_fields)]
pub enum RepresentationPayload {
    /// Canonical JSON, size-bounded and schema-validated.
    Inline { document: serde_json::Value },
    /// Content-addressed. Core stores and verifies; it never parses.
    ObjectRef {
        digest: String,
        media_type: String,
        length: u64,
    },
}

/// Neutral, presentation-facing figures a representation chose to expose.
///
/// Keys are contributed and opaque: `draft.text.document/line.added` means
/// nothing to Core beyond being the key a contributed risk rule or budget may
/// name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepresentationSummary {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metrics: BTreeMap<String, i64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

/// Where in a resource a change claims territory.
///
/// Three shapes, because domains genuinely differ. A text document has ordered
/// lines; a scene graph, a CAD assembly or a dependency graph has named nodes
/// and no meaningful ordering at all. Forcing the second into the first is what
/// makes a change-control system software-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConflictScope {
    /// The whole resource. Anything else touching it conflicts.
    Whole,
    /// A region of a contributed one-dimensional coordinate space. Core compares
    /// intervals only when both sides name the *same* space; it never learns
    /// what a "line" or a "frame" is.
    LinearRegion {
        coordinate_space: String,
        start: u64,
        length: u64,
    },
    /// A stable key in a contributed key space, for domains with no ordering.
    OpaqueKey { key_space: String, key: String },
}

/// One claim over part of a resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConflictClaim {
    pub id: String,
    pub scope: ConflictScope,
}

/// How two sets of claims relate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimRelation {
    Independent,
    Conflicting {
        reason: String,
    },
    /// Draft cannot tell. Treated as conflicting, and reported as uncertainty
    /// rather than dressed up as a decision.
    Indeterminate {
        reason: String,
    },
}

impl ClaimRelation {
    /// Whether these claims may be composed.
    ///
    /// Only `Independent` may. `Indeterminate` fails closed: when Draft cannot
    /// establish that two changes are separable, composing them would be a
    /// guess with a merge-shaped blast radius.
    pub fn is_composable(&self) -> bool {
        matches!(self, Self::Independent)
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Independent => None,
            Self::Conflicting { reason } | Self::Indeterminate { reason } => Some(reason),
        }
    }
}

/// Decide how two claim sets over the *same resource* relate.
///
/// The full matrix, and the reasoning behind each row:
///
/// | left | right | outcome |
/// |---|---|---|
/// | `Whole` | anything | conflicting — a whole-resource claim admits no neighbours |
/// | `LinearRegion{s}` | `LinearRegion{s}` | overlap decides; same space, so intervals are comparable |
/// | `LinearRegion{a}` | `LinearRegion{b}` | indeterminate — two coordinate systems Core cannot relate |
/// | `OpaqueKey{k,s}` | `OpaqueKey{k,s}` | conflicting — same key in the same space is the same thing |
/// | `OpaqueKey{k1,s}` | `OpaqueKey{k2,s}` | independent — distinct keys in one space are distinct things |
/// | `OpaqueKey{_,s1}` | `OpaqueKey{_,s2}` | indeterminate — key spaces are not comparable |
/// | `LinearRegion` | `OpaqueKey` | indeterminate — incomparable claim shapes |
/// | claims | none (but changed) | indeterminate — the silent side may have touched anything |
/// | none | none | caller falls back to whole-resource state comparison |
pub fn reconcile(left: &[ConflictClaim], right: &[ConflictClaim]) -> ClaimRelation {
    if left.is_empty() && right.is_empty() {
        return ClaimRelation::Indeterminate {
            reason: "neither change explains where it applies; compare whole-resource state".into(),
        };
    }
    if left.is_empty() || right.is_empty() {
        return ClaimRelation::Indeterminate {
            reason:
                "one change explains where it applies and the other does not, so the silent side \
                 may touch anything in this resource"
                    .into(),
        };
    }

    let mut indeterminate: Option<String> = None;
    for left_claim in left {
        for right_claim in right {
            match relate(&left_claim.scope, &right_claim.scope) {
                ClaimRelation::Conflicting { reason } => {
                    return ClaimRelation::Conflicting { reason }
                }
                ClaimRelation::Indeterminate { reason } => {
                    indeterminate.get_or_insert(reason);
                }
                ClaimRelation::Independent => {}
            }
        }
    }
    match indeterminate {
        Some(reason) => ClaimRelation::Indeterminate { reason },
        None => ClaimRelation::Independent,
    }
}

fn relate(left: &ConflictScope, right: &ConflictScope) -> ClaimRelation {
    use ConflictScope as Scope;
    match (left, right) {
        (Scope::Whole, _) | (_, Scope::Whole) => ClaimRelation::Conflicting {
            reason: "one change claims the whole resource".into(),
        },
        (
            Scope::LinearRegion {
                coordinate_space: left_space,
                start: left_start,
                length: left_length,
            },
            Scope::LinearRegion {
                coordinate_space: right_space,
                start: right_start,
                length: right_length,
            },
        ) => {
            if left_space != right_space {
                return ClaimRelation::Indeterminate {
                    reason: format!(
                        "coordinate spaces '{left_space}' and '{right_space}' are not comparable"
                    ),
                };
            }
            if intervals_overlap(*left_start, *left_length, *right_start, *right_length) {
                ClaimRelation::Conflicting {
                    reason: format!("overlapping regions in coordinate space '{left_space}'"),
                }
            } else {
                ClaimRelation::Independent
            }
        }
        (
            Scope::OpaqueKey {
                key_space: left_space,
                key: left_key,
            },
            Scope::OpaqueKey {
                key_space: right_space,
                key: right_key,
            },
        ) => {
            if left_space != right_space {
                return ClaimRelation::Indeterminate {
                    reason: format!(
                        "key spaces '{left_space}' and '{right_space}' are not comparable"
                    ),
                };
            }
            if left_key == right_key {
                ClaimRelation::Conflicting {
                    reason: format!("both changes claim key '{left_key}' in '{left_space}'"),
                }
            } else {
                ClaimRelation::Independent
            }
        }
        _ => ClaimRelation::Indeterminate {
            reason: "a linear region and an opaque key are not comparable".into(),
        },
    }
}

/// Half-open intervals: a zero-length claim marks a position, not a span.
fn intervals_overlap(
    left_start: u64,
    left_length: u64,
    right_start: u64,
    right_length: u64,
) -> bool {
    let left_end = left_start.saturating_add(left_length);
    let right_end = right_start.saturating_add(right_length);
    if left_length == 0 || right_length == 0 {
        // An insertion point conflicts only with a span that strictly contains
        // it, so two insertions at the same position stay composable.
        return (left_length == 0 && right_start < left_start && left_start < right_end)
            || (right_length == 0 && left_start < right_start && right_start < left_end);
    }
    left_start < right_end && right_start < left_end
}

/// An independently addressable unit a human may accept or reject.
///
/// Deliberately not the same thing as a conflict claim. A claim describes
/// reconciliation territory — where two changes might collide. A review unit
/// describes a decision target — what a person can say yes or no to. They often
/// coincide, and linking them is optional, but a domain may reasonably have one
/// without the other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewUnit {
    /// Stable across re-derivation of the same representation, so a decision
    /// made against it survives a regeneration that did not change it.
    pub unit_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub summary: BTreeMap<String, i64>,
    /// Optional linkage, never identity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflict_claim_ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(id: &str, scope: ConflictScope) -> ConflictClaim {
        ConflictClaim {
            id: id.into(),
            scope,
        }
    }

    fn line(start: u64, length: u64) -> ConflictScope {
        ConflictScope::LinearRegion {
            coordinate_space: "draft.text.document/line".into(),
            start,
            length,
        }
    }

    fn key(space: &str, value: &str) -> ConflictScope {
        ConflictScope::OpaqueKey {
            key_space: space.into(),
            key: value.into(),
        }
    }

    #[test]
    fn a_whole_resource_claim_admits_no_neighbours() {
        let whole = [claim("a", ConflictScope::Whole)];
        let region = [claim("b", line(10, 2))];
        assert!(matches!(
            reconcile(&whole, &region),
            ClaimRelation::Conflicting { .. }
        ));
        assert!(matches!(
            reconcile(&region, &whole),
            ClaimRelation::Conflicting { .. }
        ));
    }

    #[test]
    fn regions_in_one_space_compose_when_they_do_not_overlap() {
        // The behaviour fine-grained review depends on: two edits to the same
        // resource, in different places, are separable.
        let left = [claim("a", line(1, 3))];
        let right = [claim("b", line(10, 3))];
        assert!(reconcile(&left, &right).is_composable());

        let overlapping = [claim("b", line(2, 5))];
        assert!(matches!(
            reconcile(&left, &overlapping),
            ClaimRelation::Conflicting { .. }
        ));
    }

    #[test]
    fn regions_in_different_spaces_are_indeterminate_not_independent() {
        let lines = [claim("a", line(1, 3))];
        let frames = [claim(
            "b",
            ConflictScope::LinearRegion {
                coordinate_space: "example.media/frame".into(),
                start: 1,
                length: 3,
            },
        )];
        let relation = reconcile(&lines, &frames);
        assert!(matches!(relation, ClaimRelation::Indeterminate { .. }));
        // Fails closed: unknown is not permission to compose.
        assert!(!relation.is_composable());
        assert!(relation.reason().unwrap().contains("not comparable"));
    }

    #[test]
    fn opaque_keys_decide_by_equality_within_one_space() {
        let space = "example.scene/node";
        assert!(reconcile(
            &[claim("a", key(space, "node-1"))],
            &[claim("b", key(space, "node-2"))]
        )
        .is_composable());
        assert!(matches!(
            reconcile(
                &[claim("a", key(space, "node-1"))],
                &[claim("b", key(space, "node-1"))]
            ),
            ClaimRelation::Conflicting { .. }
        ));
        // Two key spaces are two vocabularies; equality across them means nothing.
        assert!(matches!(
            reconcile(
                &[claim("a", key("example.scene/node", "x"))],
                &[claim("b", key("example.cad/part", "x"))]
            ),
            ClaimRelation::Indeterminate { .. }
        ));
    }

    #[test]
    fn incomparable_claim_shapes_are_indeterminate() {
        assert!(matches!(
            reconcile(
                &[claim("a", line(1, 3))],
                &[claim("b", key("example.scene/node", "n"))]
            ),
            ClaimRelation::Indeterminate { .. }
        ));
    }

    #[test]
    fn a_silent_side_is_uncertainty_not_independence() {
        // Without an explanation from one side, Draft cannot know it stayed out
        // of the way — and must not assume it did.
        let claims = [claim("a", line(1, 3))];
        let relation = reconcile(&claims, &[]);
        assert!(matches!(relation, ClaimRelation::Indeterminate { .. }));
        assert!(!relation.is_composable());
        assert!(matches!(
            reconcile(&[], &[]),
            ClaimRelation::Indeterminate { .. }
        ));
    }

    #[test]
    fn insertion_points_at_one_position_stay_composable() {
        // Two zero-length claims at the same position are two insertions, not a
        // collision; treating them as overlapping would block ordinary
        // fine-grained review.
        assert!(reconcile(&[claim("a", line(5, 0))], &[claim("b", line(5, 0))]).is_composable());
        // An insertion strictly inside another change's span does collide.
        assert!(matches!(
            reconcile(&[claim("a", line(5, 0))], &[claim("b", line(3, 5))]),
            ClaimRelation::Conflicting { .. }
        ));
        // And one at the boundary does not.
        assert!(reconcile(&[claim("a", line(3, 0))], &[claim("b", line(3, 5))]).is_composable());
    }
}
