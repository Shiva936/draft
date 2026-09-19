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

use draft_dcg_contract::ids::ChangePackId;

use crate::app::Workspace;
use crate::dcg::compose::{
    compose as compose_revisions, disperse as disperse_composition, ComposedRevision, Composition,
    DispersedRevision, PairwiseRelation, Relationship,
};
use crate::evidence::representation::{
    interference, InterferenceRelation, RepresentationStore, RevisionPackRepresentationBundle,
};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// The newest sealed revision of a ChangePack, as a composition member.
pub fn member(workspace: &Workspace, change: &ChangePackId) -> DraftResult<ComposedRevision> {
    let view = crate::app::workflow::change_pack_views(workspace)?
        .into_iter()
        .find(|view| &view.change_pack == change)
        .ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::NotFound,
                format!("no ChangePack '{change}'"),
            )
        })?;
    let revision = view.revisions.first().cloned().ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::NotFound,
            format!(
                "ChangePack {change} has sealed no revision, so what it touches is not yet a fact"
            ),
        )
        .with_suggestion("Seal a revision on every ChangePack you want to compose.")
    })?;
    Ok(ComposedRevision {
        change_pack: change.clone(),
        revision_pack: revision.id.clone(),
        base_baseline: revision.base_baseline.clone(),
        touched: revision.touched.clone(),
    })
}

/// Compose the newest sealed revisions of several ChangePacks.
pub fn compose(workspace: &Workspace, changes: &[ChangePackId]) -> DraftResult<Composition> {
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
                "a composition needs at least two ChangePacks",
            )
        })?;

    let store = RepresentationStore::new(workspace.layout.representations_dir());
    let bundles: Vec<Option<RevisionPackRepresentationBundle>> = members
        .iter()
        .map(|member| store.get(&member.revision_pack))
        .collect::<DraftResult<_>>()?;
    let bundle_of = |member: &ComposedRevision| {
        members
            .iter()
            .position(|candidate| candidate.revision_pack == member.revision_pack)
            .and_then(|index| bundles[index].as_ref())
    };

    compose_revisions(&base, &members, |left, right| {
        relate(left, bundle_of(left), right, bundle_of(right))
    })
}

/// Take a composition apart into revisions that can move separately.
pub fn disperse(
    workspace: &Workspace,
    changes: &[ChangePackId],
) -> DraftResult<Vec<DispersedRevision>> {
    Ok(disperse_composition(&compose(workspace, changes)?))
}

/// Every other ChangePack whose newest revision interferes with this one's.
pub fn conflicts(
    workspace: &Workspace,
    change: &ChangePackId,
) -> DraftResult<Vec<PairwiseRelation>> {
    let subject = member(workspace, change)?;
    let store = RepresentationStore::new(workspace.layout.representations_dir());
    let subject_bundle = store.get(&subject.revision_pack)?;

    let mut found = Vec::new();
    for view in crate::app::workflow::change_pack_views(workspace)? {
        if &view.change_pack == change {
            continue;
        }
        // A ChangePack with no sealed revision has touched nothing yet, so there
        // is nothing to interfere with. Reporting it as a conflict would make
        // every open ChangePack look like an obstacle.
        let Some(revision) = view.revisions.first() else {
            continue;
        };
        let other = ComposedRevision {
            change_pack: view.change_pack.clone(),
            revision_pack: revision.id.clone(),
            base_baseline: revision.base_baseline.clone(),
            touched: revision.touched.clone(),
        };
        let other_bundle = store.get(&other.revision_pack)?;
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
    left_bundle: Option<&RevisionPackRepresentationBundle>,
    right: &ComposedRevision,
    right_bundle: Option<&RevisionPackRepresentationBundle>,
) -> PairwiseRelation {
    // Different starting points first: disjoint Resource sets prove nothing
    // when the two revisions were worked from different Baselines, whatever
    // the representations say about where inside a Resource each landed.
    if left.base_baseline != right.base_baseline {
        return PairwiseRelation {
            left: left.revision_pack.clone(),
            right: right.revision_pack.clone(),
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
            left: left.revision_pack.clone(),
            right: right.revision_pack.clone(),
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
        left: left.revision_pack.clone(),
        right: right.revision_pack.clone(),
        relation: if indeterminate {
            Relationship::Indeterminate
        } else {
            Relationship::Conflicting
        },
        detail,
        shared_resources: shared,
    }
}
