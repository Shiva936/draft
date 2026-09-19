//! Whether a promotion's coverage is good enough to accept.
//!
//! # Why absence is not evidence
//!
//! A promotion accepts a Baseline as the project's state. That claim is only
//! as good as the observation behind it, and an observation that could not see
//! part of the project has not established that part is empty — it has
//! established nothing about it.
//!
//! The failure this prevents is quiet: a binding that failed to enumerate a
//! domain produces the same *resource set* as one that enumerated it and found
//! nothing. Accepting a Baseline on that basis records "these are the
//! resources" when the truth is "these are the resources we could see".
//!
//! So `NotObserved` never satisfies a required domain, and the reason is
//! reported: a domain nobody attempted and a domain whose attempt failed call
//! for different responses — configure something, or fix something.

use draft_dcg_contract::coverage::{CoverageEvidence, CoverageStatus};
use std::collections::BTreeSet;

/// Why coverage was not sufficient for a required domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverageShortfall {
    /// Nothing claimed this domain at all.
    Missing { domain: String },
    /// A claim exists but the domain was never attempted.
    ///
    /// Usually configuration: nothing is bound for it.
    NotAttempted { domain: String },
    /// The domain was attempted and the attempt failed.
    ///
    /// Distinct from `NotAttempted` because the fix is different, and because
    /// a failed attempt is a signal about the binding that "not configured"
    /// is not.
    AttemptFailed { domain: String },
    /// The domain was enumerated but known gaps remain.
    Incomplete { domain: String },
}

impl CoverageShortfall {
    pub fn domain(&self) -> &str {
        match self {
            Self::Missing { domain }
            | Self::NotAttempted { domain }
            | Self::AttemptFailed { domain }
            | Self::Incomplete { domain } => domain,
        }
    }
}

/// Check coverage against the domains policy requires.
///
/// Returns every shortfall rather than the first. A promoter fixing coverage
/// needs the whole list: reporting one at a time turns a single configuration
/// problem into a sequence of failed promotions.
///
/// `required` is what policy demands. An empty requirement set is satisfied by
/// anything — including no coverage at all — which is a decision policy makes
/// explicitly rather than something this function infers.
pub fn shortfalls(
    evidence: &[CoverageEvidence],
    required: &BTreeSet<String>,
) -> Vec<CoverageShortfall> {
    let mut found = Vec::new();
    for domain in required {
        let claim = evidence
            .iter()
            .find(|entry| entry.domain.as_str() == domain.as_str());
        match claim {
            None => found.push(CoverageShortfall::Missing {
                domain: domain.clone(),
            }),
            Some(entry) => match entry.status {
                CoverageStatus::Complete => {}
                CoverageStatus::Incomplete => found.push(CoverageShortfall::Incomplete {
                    domain: domain.clone(),
                }),
                CoverageStatus::NotObserved => {
                    // The contract records `attempted` directly rather than
                    // leaving it to be inferred, so this reads the fact instead
                    // of deducing it from whether a run reference survived.
                    if entry.attempted {
                        found.push(CoverageShortfall::AttemptFailed {
                            domain: domain.clone(),
                        });
                    } else {
                        found.push(CoverageShortfall::NotAttempted {
                            domain: domain.clone(),
                        });
                    }
                }
            },
        }
    }
    found
}

/// Whether coverage satisfies policy.
pub fn satisfies(evidence: &[CoverageEvidence], required: &BTreeSet<String>) -> bool {
    shortfalls(evidence, required).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_dcg_contract::coverage::CoverageDomainRef;
    use draft_dcg_contract::ids::{ObservationRunId, ProviderBindingId};
    use draft_dcg_contract::observation::{ObservationRunDigest, ObservationRunRef};
    use draft_dcg_contract::{Digest, ProviderSemanticDefinitionDigest};

    fn claim(domain: &str, status: CoverageStatus, attempted: bool) -> CoverageEvidence {
        CoverageEvidence {
            attempted,
            committed: status == CoverageStatus::Complete,
            known_gaps: Default::default(),
            provider_binding: ProviderBindingId::parse("pbd_000000000001").unwrap(),
            provider_semantic_definition: ProviderSemanticDefinitionDigest::new(Digest::of_bytes(
                b"semantics",
            )),
            domain: CoverageDomainRef::parse(domain).unwrap(),
            status,
            observation_run: attempted.then(|| ObservationRunRef {
                id: ObservationRunId::parse("run_000000000001").unwrap(),
                digest: ObservationRunDigest::new(Digest::of_bytes(b"run")),
            }),
        }
    }

    fn required(domains: &[&str]) -> BTreeSet<String> {
        domains.iter().map(|d| d.to_string()).collect()
    }

    #[test]
    fn complete_coverage_of_every_required_domain_satisfies() {
        let evidence = vec![
            claim("files", CoverageStatus::Complete, true),
            claim("records", CoverageStatus::Complete, true),
        ];
        assert!(satisfies(&evidence, &required(&["files", "records"])));
    }

    #[test]
    fn a_failed_attempt_and_an_unattempted_domain_are_different_shortfalls() {
        // Both produce the same empty resource set. Reporting them alike would
        // send someone to fix a binding that was never configured, or to
        // configure one that is already failing.
        let evidence = vec![
            claim("files", CoverageStatus::NotObserved, true),
            claim("records", CoverageStatus::NotObserved, false),
        ];
        let found = shortfalls(&evidence, &required(&["files", "records"]));
        assert_eq!(
            found,
            vec![
                CoverageShortfall::AttemptFailed {
                    domain: "files".into()
                },
                CoverageShortfall::NotAttempted {
                    domain: "records".into()
                },
            ]
        );
    }

    #[test]
    fn an_unobserved_domain_never_satisfies_by_looking_empty() {
        // The quiet failure: a binding that could not enumerate a domain
        // yields the same resources as one that enumerated it and found none.
        let evidence = vec![claim("files", CoverageStatus::NotObserved, true)];
        assert!(!satisfies(&evidence, &required(&["files"])));
    }

    #[test]
    fn a_domain_nothing_claimed_at_all_is_missing() {
        assert_eq!(
            shortfalls(&[], &required(&["files"])),
            vec![CoverageShortfall::Missing {
                domain: "files".into()
            }]
        );
    }

    #[test]
    fn every_shortfall_is_reported_not_just_the_first() {
        // A promoter fixing coverage needs the whole list; one at a time turns
        // a single misconfiguration into a sequence of failed promotions.
        let evidence = vec![claim("files", CoverageStatus::Incomplete, true)];
        let found = shortfalls(&evidence, &required(&["files", "records", "timeline"]));
        assert_eq!(found.len(), 3);
        assert_eq!(
            found.iter().map(|s| s.domain()).collect::<Vec<_>>(),
            vec!["files", "records", "timeline"]
        );
    }

    #[test]
    fn requiring_nothing_is_satisfied_by_nothing() {
        // Policy's decision to require no domains is explicit, not inferred.
        assert!(satisfies(&[], &BTreeSet::new()));
    }
}
