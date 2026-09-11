//! One accepted Baseline, as §8.3's Baseline scope shows it.
//!
//! Every field here is a separate question the plan insists on keeping
//! separate: what material state is accepted, what provenance establishes it,
//! what coverage justifies absence, where it came from, what composed it,
//! whether it could be recovered, what attested it, and what was delivered
//! from it.
//!
//! # Publication is not Baseline
//!
//! Publications are carried here because a reader looking at an accepted
//! Baseline wants to know what was delivered from it — but they are their own
//! field, with their own vocabulary, and nothing in this view lets a delivery
//! change what the project accepts. Promotion is the only thing that moves a
//! Baseline; a failed publication leaves this view identical.

use std::collections::BTreeMap;

use draft_dcg_contract::baseline::{BaselineId, BaselineManifest};
use draft_dcg_contract::ids::ResourceId;
use draft_dcg_contract::provider::ProviderProvenanceRef;
use serde::Serialize;

use crate::app::workflow::PublicationView;
use crate::dcg::anchor::SnapshotRecoveryStatus;
use crate::dcg::baseline::BaselineRecord;
use crate::project::Workspace;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// One recovery target, with the anchors' own verdict on it.
#[derive(Debug, Clone, Serialize)]
pub struct RecoveryTargetView {
    pub snapshot: String,
    pub created_at: String,
    /// The authoritative status, computed by [`crate::dcg::anchor::RecoveryAnchorSet::status`].
    /// Never derived from whether an anchor file happens to exist.
    pub status: SnapshotRecoveryStatus,
}

/// What could actually be restored, and from where.
///
/// Asked of the anchors, never inferred from a file being present. `Snapshot
/// complete` never means `snapshot restorable`, so the two are reported as
/// separate facts and an unanchored target is listed rather than omitted.
#[derive(Debug, Clone, Serialize)]
pub struct RecoverabilityView {
    /// Newest first. Every target, including the ones nothing can restore.
    pub targets: Vec<RecoveryTargetView>,
    /// How many of them the anchors cover completely.
    pub fully_anchored: usize,
    /// Said plainly rather than left to a ratio a reader has to interpret.
    pub summary: String,
}

/// One accepted Baseline and everything §8.3 renders about it.
#[derive(Debug, Clone, Serialize)]
pub struct BaselineDetailView {
    pub baseline: BaselineId,
    /// Whether this is the Baseline the project currently accepts.
    pub accepted: bool,
    pub manifest: BaselineManifest,
    pub record: BaselineRecord,
    /// This Baseline back to the project's first, newest first.
    pub lineage: Vec<BaselineId>,
    /// What established each Resource's accepted state. Fixed at acceptance
    /// and never changed by a later unbind or reprofile.
    pub composition: BTreeMap<ResourceId, ProviderProvenanceRef>,
    pub recoverability: RecoverabilityView,
    /// Receipts attesting the promotion that accepted this Baseline.
    pub receipts: Vec<serde_json::Value>,
    /// What was delivered from this Baseline, per target. Separate from the
    /// Baseline itself in every sense that matters.
    pub publications: Vec<PublicationView>,
    /// Whether a Publication of this Baseline could be routed right now.
    ///
    /// Current configuration, not history: an unroutable Baseline is still
    /// fully accepted, and only delivery is blocked.
    pub routable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_refusal: Option<String>,
}

/// Every Baseline in the accepted lineage, newest first.
pub fn list(workspace: &Workspace) -> DraftResult<Vec<BaselineDetailView>> {
    let Some(current) = crate::dcg::baseline::current_baseline(&workspace.layout)? else {
        return Ok(Vec::new());
    };
    let store = crate::dcg::baseline::BaselineStore::new(workspace.layout.baselines_dir());
    store
        .lineage(&current)?
        .into_iter()
        .map(|baseline| detail(workspace, &baseline))
        .collect()
}

/// One Baseline, with its roots, lineage, composition and deliveries.
pub fn detail(workspace: &Workspace, baseline: &BaselineId) -> DraftResult<BaselineDetailView> {
    let store = crate::dcg::baseline::BaselineStore::new(workspace.layout.baselines_dir());
    let manifest = store.manifest(baseline)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::NotFound,
            format!("this project holds no manifest for Baseline '{baseline}'"),
        )
    })?;
    let record = store.record(baseline)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::NotFound,
            format!("this project holds no record for Baseline '{baseline}'"),
        )
    })?;
    let composition = store
        .composition(baseline)?
        .map(|value| value.resource_provenance)
        .unwrap_or_default();
    let current = crate::dcg::baseline::current_baseline(&workspace.layout)?;
    let accepted = current.as_ref() == Some(baseline);

    // Routability is a current-configuration question and is only meaningful
    // for the Baseline that could actually be published.
    let (routable, route_refusal) =
        match crate::app::publish::route_for_baseline(workspace, baseline) {
            Ok(_) => (true, None),
            Err(error) => (false, Some(error.message.clone())),
        };

    let publications = crate::app::workflow::publication_views(workspace)?
        .into_iter()
        .filter(|view| &view.baseline == baseline)
        .collect();

    let rendered = baseline.to_string();
    let receipts = crate::receipt::ReceiptEnvelopeStore::for_layout(&workspace.layout)
        .read_all()?
        .into_iter()
        .map(|envelope| serde_json::to_value(envelope).map_err(DraftError::from))
        .collect::<DraftResult<Vec<serde_json::Value>>>()?
        .into_iter()
        .filter(|envelope| {
            crate::support::hashing::canonical_json(envelope).contains(rendered.as_str())
        })
        .collect();

    Ok(BaselineDetailView {
        recoverability: recoverability(workspace)?,
        baseline: baseline.clone(),
        accepted,
        manifest,
        record,
        lineage: store.lineage(baseline)?,
        composition,
        receipts,
        publications,
        routable,
        route_refusal,
    })
}

/// What the anchors say about restoring this project's state.
///
/// Delegated whole to [`crate::dcg::anchor::RecoveryAnchorSet::status`]: the
/// Console must not decide for itself what is restorable, and a second
/// implementation of that judgement is exactly how a frontend ends up
/// promising a restore Draft cannot perform.
fn recoverability(workspace: &Workspace) -> DraftResult<RecoverabilityView> {
    let mut targets = Vec::new();
    for snapshot in snapshots(workspace)? {
        let status = crate::app::anchor_set_for(workspace, &snapshot)?.status(&snapshot);
        targets.push(RecoveryTargetView {
            snapshot: snapshot.id.to_string(),
            created_at: snapshot.created_at.to_rfc3339(),
            status,
        });
    }
    targets.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    let fully_anchored = targets
        .iter()
        .filter(|target| target.status.is_fully_anchored())
        .count();
    Ok(RecoverabilityView {
        summary: match (targets.len(), fully_anchored) {
            (0, _) => "no recovery target has been captured for this project".to_string(),
            (total, 0) => format!(
                "{total} recovery target(s) exist and none is fully anchored; nothing here is \
                 provably restorable"
            ),
            (total, anchored) => format!(
                "{anchored} of {total} recovery target(s) are fully anchored; the rest cannot be \
                 proved restorable"
            ),
        },
        fully_anchored,
        targets,
    })
}

/// Every captured snapshot, in storage order.
fn snapshots(workspace: &Workspace) -> DraftResult<Vec<crate::dcg::state::Snapshot>> {
    let directory = workspace.layout.snapshots_dir();
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(DraftError::storage(format!(
                "cannot list snapshots in {}: {error}",
                directory.display()
            )))
        }
    };
    let mut snapshots = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            snapshots.push(crate::contracts::read_persisted(&path)?);
        }
    }
    Ok(snapshots)
}
