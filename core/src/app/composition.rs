//! Composing sealed revisions, and taking a composition apart again.
//!
//! # Why the orchestration is here
//!
//! `dcg::compose` owns the algebra and knows nothing about representations:
//! the graph must stay readable with nothing installed. The finest answer to
//! "do these two revisions interfere?" needs the explanations that live above
//! the graph, so the two are joined here, where both are reachable.
//!
//! The conservative direction always wins. A pair Draft cannot show separable
//! is reported as interfering, and a composition holds only when every pair is
//! independent and every member was sealed from the same Baseline.

use std::collections::BTreeSet;

use draft_dcg_contract::ids::ChangeId;

use crate::app::Workspace;
use crate::dcg::compose::{
    compose as compose_revisions, disperse as disperse_composition, ComposedRevision, Composition,
    DispersedRevision, PairwiseRelation, Relationship,
};
use crate::evidence::representation::{
    interference, ChangeRepresentationBundle, InterferenceRelation, RepresentationStore,
};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// The newest sealed revision of a Change, as a composition member.
pub fn member(workspace: &Workspace, change: &ChangeId) -> DraftResult<ComposedRevision> {
    let view = crate::app::workflow::change_views(workspace)?
        .into_iter()
        .find(|view| &view.change == change)
        .ok_or_else(|| {
            DraftError::new(DraftErrorKind::NotFound, format!("no Change '{change}'"))
        })?;
    let revision = view.revisions.first().cloned().ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::NotFound,
            format!("Change {change} has sealed no revision, so what it touches is not yet a fact"),
        )
        .with_suggestion("Seal a revision on every Change you want to compose.")
    })?;
    Ok(ComposedRevision {
        change: change.clone(),
        revision: revision.id.clone(),
        base_baseline: revision.base_baseline.clone(),
        touched: revision.touched.clone(),
    })
}

/// Compose the newest sealed revisions of several Changes.
pub fn compose(workspace: &Workspace, changes: &[ChangeId]) -> DraftResult<Composition> {
    let members: Vec<ComposedRevision> = changes
        .iter()
        .map(|change| member(workspace, change))
        .collect::<DraftResult<_>>()?;
    let base = members
        .first()
        .map(|first| first.base_baseline.clone())
        .ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::Validation,
                "a composition needs at least two Changes",
            )
        })?;

    let store = RepresentationStore::new(workspace.layout.representations_dir());
    let bundles: Vec<Option<ChangeRepresentationBundle>> = members
        .iter()
        .map(|member| store.get(&member.revision))
        .collect::<DraftResult<_>>()?;
    let bundle_of = |member: &ComposedRevision| {
        members
            .iter()
            .position(|candidate| candidate.revision == member.revision)
            .and_then(|index| bundles[index].as_ref())
    };

    compose_revisions(&base, &members, |left, right| {
        relate(left, bundle_of(left), right, bundle_of(right))
    })
}

/// Take a composition apart into revisions that can move separately.
pub fn disperse(
    workspace: &Workspace,
    changes: &[ChangeId],
) -> DraftResult<Vec<DispersedRevision>> {
    Ok(disperse_composition(&compose(workspace, changes)?))
}

/// Every other Change whose newest revision interferes with this one's.
pub fn conflicts(workspace: &Workspace, change: &ChangeId) -> DraftResult<Vec<PairwiseRelation>> {
    let subject = member(workspace, change)?;
    let store = RepresentationStore::new(workspace.layout.representations_dir());
    let subject_bundle = store.get(&subject.revision)?;

    let mut found = Vec::new();
    for view in crate::app::workflow::change_views(workspace)? {
        if &view.change == change {
            continue;
        }
        // A Change with no sealed revision has touched nothing yet, so there
        // is nothing to interfere with. Reporting it as a conflict would make
        // every open Change look like an obstacle.
        let Some(revision) = view.revisions.first() else {
            continue;
        };
        let other = ComposedRevision {
            change: view.change.clone(),
            revision: revision.id.clone(),
            base_baseline: revision.base_baseline.clone(),
            touched: revision.touched.clone(),
        };
        let other_bundle = store.get(&other.revision)?;
        let relation = relate(
            &subject,
            subject_bundle.as_ref(),
            &other,
            other_bundle.as_ref(),
        );
        if !relation.relation.is_composable() {
            found.push(relation);
        }
    }
    Ok(found)
}

/// How one pair stands, using representations where both sides have them.
pub fn relate(
    left: &ComposedRevision,
    left_bundle: Option<&ChangeRepresentationBundle>,
    right: &ComposedRevision,
    right_bundle: Option<&ChangeRepresentationBundle>,
) -> PairwiseRelation {
    // Different starting points first: disjoint Resource sets prove nothing
    // when the two revisions were worked from different Baselines, whatever
    // the representations say about where inside a Resource each landed.
    if left.base_baseline != right.base_baseline {
        return PairwiseRelation {
            left: left.revision.clone(),
            right: right.revision.clone(),
            relation: Relationship::Indeterminate,
            detail: "sealed from different Baselines, so disjoint Resource sets prove nothing"
                .to_string(),
            shared_resources: left.touched.intersection(&right.touched).cloned().collect(),
        };
    }

    let findings = interference(&left.touched, left_bundle, &right.touched, right_bundle);
    let shared: BTreeSet<_> = findings
        .iter()
        .map(|finding| finding.resource_id.clone())
        .collect();
    if findings.is_empty() {
        return PairwiseRelation {
            left: left.revision.clone(),
            right: right.revision.clone(),
            relation: Relationship::Independent,
            detail: String::new(),
            shared_resources: shared,
        };
    }
    // Indeterminate dominates: "we could not tell" is a weaker claim than "we
    // can see them collide", and reporting the stronger one would overstate
    // what Draft established.
    let indeterminate = findings
        .iter()
        .any(|finding| finding.relation == InterferenceRelation::Indeterminate);
    let detail = findings
        .iter()
        .map(|finding| {
            if finding.detail.is_empty() {
                finding.resource_id.to_string()
            } else {
                format!("{}: {}", finding.resource_id, finding.detail)
            }
        })
        .collect::<Vec<_>>()
        .join("; ");
    PairwiseRelation {
        left: left.revision.clone(),
        right: right.revision.clone(),
        relation: if indeterminate {
            Relationship::Indeterminate
        } else {
            Relationship::Conflicting
        },
        detail,
        shared_resources: shared,
    }
}
