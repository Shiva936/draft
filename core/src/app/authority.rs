//! Reading and withdrawing what this project is permitted to do.
//!
//! # Issued and in force are different questions
//!
//! A grant is an immutable fact: somebody permitted somebody else to do
//! something, and that stays true however the project later changes its mind.
//! Whether it currently confers anything is the project's security state's
//! answer, read under the control lock.
//!
//! Listing folds both and keeps them apart. A grant that has been revoked
//! still appears, marked revoked, because a record of authority that quietly
//! drops what was withdrawn cannot answer the question an audit actually asks.
//!
//! # Revoking is a fact, not a deletion
//!
//! `revoke` writes an immutable [`AuthorityRevocation`] naming the grant by
//! exact reference, then moves that reference from the active set into the
//! revocations set under the control lock. Nothing is deleted, and a receipt
//! that cited the grant while it was live stays valid history — a later
//! revocation blocks new operations and never rewrites old ones.

use draft_dcg_contract::ids::AuthorityGrantId;
use draft_dcg_contract::security::SecurityFactRef;
use serde::Serialize;

use crate::authority::grant::AuthorityGrant;
use crate::authority::revocation::AuthorityRevocation;
use crate::project::Workspace;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};

/// One grant, and whether it is currently in force.
#[derive(Debug, Clone, Serialize)]
pub struct GrantView {
    pub grant: AuthorityGrant,
    /// The exact reference the project's security state would name.
    pub reference: SecurityFactRef,
    pub active: bool,
    pub revoked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revocation: Option<AuthorityRevocation>,
}

fn stores(workspace: &Workspace) -> DraftResult<crate::publication::authority::AuthorityStores> {
    crate::publication::authority::AuthorityStores::for_layout(&workspace.layout)
}

fn security_state(
    stores: &crate::publication::authority::AuthorityStores,
) -> DraftResult<crate::project::security::ProjectSecurityState> {
    let control = stores.control.read_unlocked()?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::CorruptData,
            "this project has no control record",
        )
    })?;
    Ok(stores
        .security_states
        .get(&control.project_security_state)?
        .unwrap_or_default())
}

/// Every grant this project has issued, with its current standing.
pub fn list(workspace: &Workspace) -> DraftResult<Vec<GrantView>> {
    let stores = stores(workspace)?;
    let state = security_state(&stores)?;
    let mut views = Vec::new();
    for grant in stores.grants.list()? {
        let reference = grant.reference()?;
        views.push(GrantView {
            active: state.is_active(&reference),
            revoked: state.is_revoked(&reference),
            revocation: stores.revocations.get(&grant.id)?,
            reference,
            grant,
        });
    }
    Ok(views)
}

/// One grant, with its current standing.
pub fn show(workspace: &Workspace, id: &AuthorityGrantId) -> DraftResult<GrantView> {
    let stores = stores(workspace)?;
    let grant = stores
        .grants
        .get(id)?
        .ok_or_else(|| DraftError::new(DraftErrorKind::NotFound, format!("no grant '{id}'")))?;
    let state = security_state(&stores)?;
    let reference = grant.reference()?;
    Ok(GrantView {
        active: state.is_active(&reference),
        revoked: state.is_revoked(&reference),
        revocation: stores.revocations.get(&grant.id)?,
        reference,
        grant,
    })
}

/// Withdraw a grant.
///
/// The revocation is written first and the security state moved second, both
/// against the exact reference. A state naming a revocation whose fact is not
/// stored would leave the project unable to explain why something is refused.
pub fn revoke(
    workspace: &Workspace,
    id: &AuthorityGrantId,
    reason: &str,
) -> DraftResult<AuthorityRevocation> {
    if reason.trim().is_empty() {
        return Err(DraftError::new(
            DraftErrorKind::Validation,
            "a revocation needs a reason: 'revoked for cause' and 'revoked because the project \
             finished' lead to different follow-up, and neither is recoverable from the bare fact \
             that a revocation exists",
        ));
    }
    let stores = stores(workspace)?;
    let grant = stores
        .grants
        .get(id)?
        .ok_or_else(|| DraftError::new(DraftErrorKind::NotFound, format!("no grant '{id}'")))?;
    let reference = grant.reference()?;
    let actor = crate::app::baseline::actor_id_of(&workspace.layout)?;

    let revocation = AuthorityRevocation {
        grant: reference.clone(),
        grant_id: grant.id.clone(),
        revoked_by: actor,
        // Frozen, so re-running converges on the revocation already recorded
        // rather than minting a second one that says the same thing.
        revoked_at: draft_dcg_contract::value::Timestamp::from_unix_nanos(0),
        reason: reason.to_string(),
    };
    stores.revocations.put(&revocation)?;
    let revocation_reference = revocation.reference()?;

    stores.control.with_locked_control(|control| {
        let current = control.current()?.ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                "this project has no control record to revoke against",
            )
        })?;
        let security = stores
            .security_states
            .get(&current.project_security_state)?
            .unwrap_or_default();
        if security.is_revoked(&reference) {
            return Ok(());
        }
        let revoked = security.revoke(reference.clone());
        crate::app::security::structurally_valid(&revoked)?;
        let digest = stores.security_states.put(&revoked)?;
        let expected = crate::support::record_guard::ExpectedRecordState::of(&current)?;
        let advanced = current.advanced(|next| {
            next.project_security_state = digest;
        });
        control.compare_exchange_locked(&expected, &advanced)
    })?;

    // Recorded after the transition committed. An event announcing a
    // withdrawal the compare-exchange then refused would be a durable claim
    // about a state the project was never in.
    crate::app::activity::ProjectActivity::new(workspace.layout.clone(), &workspace.workspace_id)
        .append(
        crate::activity::EventKind::AuthorityRevoked,
        Some(grant.id.to_string()),
        serde_json::json!({
            "grant": grant.id.to_string(),
            "capability": grant.capability.to_string(),
            "revocation": revocation_reference.digest.to_string(),
            "reason": reason,
        }),
    )?;
    Ok(revocation)
}
