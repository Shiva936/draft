//! Whole-resource comparison.
//!
//! The universally available engine: it needs no domain knowledge at all,
//! because "the state digest changed" is true in every domain. It claims the
//! whole resource, which is the honest claim — without a finer explanation
//! Draft cannot say two changes to this resource are separable.

use super::{EngineOutput, EngineUnit};
use crate::dcg::representation::{ConflictClaim, ConflictScope};

pub const REVISION: u32 = 1;

/// Produce the whole-resource claim for one changed resource.
pub fn compare(
    before_state_digest: Option<&str>,
    after_state_digest: Option<&str>,
) -> EngineOutput {
    let mut metrics = std::collections::BTreeMap::new();
    metrics.insert(
        "draft.core/resources_claimed".to_string(),
        i64::from(before_state_digest != after_state_digest),
    );
    EngineOutput {
        units: vec![EngineUnit {
            unit_id: "whole".to_string(),
            scope: ConflictScope::Whole,
            label: "whole resource".to_string(),
        }],
        claims: vec![ConflictClaim {
            id: "whole".to_string(),
            scope: ConflictScope::Whole,
        }],
        metrics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whole_resource_claim_admits_no_neighbours() {
        let output = compare(Some("sha256:a"), Some("sha256:b"));
        assert_eq!(output.claims.len(), 1);
        assert_eq!(output.claims[0].scope, ConflictScope::Whole);
        assert_eq!(
            crate::dcg::representation::reconcile(&output.claims, &output.claims),
            crate::dcg::representation::ClaimRelation::Conflicting {
                reason: "one change claims the whole resource".to_string()
            }
        );
    }

    #[test]
    fn it_works_with_no_bytes_at_all() {
        // The point of this engine: digests are enough, so it is available for
        // a resource whose content Draft may not even be able to read.
        let output = compare(None, Some("sha256:new"));
        assert_eq!(output.units.len(), 1);
        assert_eq!(output.metrics.get("draft.core/resources_claimed"), Some(&1));
    }
}
