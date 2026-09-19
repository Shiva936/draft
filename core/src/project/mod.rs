//! The project: its identity, where its state lives, and what it accepts.
//!
//! `project` is the layer beneath the graph. It owns the directory layout, the
//! global store, configuration, the extension registry, protected paths and
//! ownership — the infrastructure a project *is stored in*, as distinct from
//! the graph of what is in it, which is `dcg`.
//!
//! It knows nothing about extensions or trust, and that is enforced rather than
//! intended: `scripts/check-core-architecture.sh` proves the module builds and
//! tests with `core::extension` absent, so the platform's base cannot quietly
//! grow a dependency on what happens to be installed.

pub mod config;
pub mod control;
pub mod credential;
pub mod home;
pub mod initialization;
pub mod layout;
pub mod object_store;
pub mod ownership;
pub mod policy;
pub mod protected;
pub mod provider;
pub mod provider_definition;
pub mod registry;
pub mod security;

pub use control::{
    ProjectControlGuard, ProjectControlState, ProjectControlStore, ProjectLifecycle,
};
pub use credential::ProviderCredentialHandle;
pub use initialization::{mint_project_id, StagedProject};
pub use layout::DraftLayout;
pub use provider::{
    ProviderBinding, ProviderBindingGuard, ProviderBindingLifecycle, ProviderBindingStore,
    RouteRefusal,
};
pub use provider_definition::{
    ConcurrencyPolicy, MergeCapability, ProviderDefinitionStore, ProviderOperationalProfile,
    ProviderSemanticDefinition, PublicationDelivery,
};
pub use security::ProjectSecurityState;

use chrono::{DateTime, Utc};
use draft_dcg_contract::ids::ProjectId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// An open project on disk.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub workspace_id: ProjectId,
    pub root: PathBuf,
    pub layout: DraftLayout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMetadata {
    pub schema_version: u32,
    pub workspace_id: ProjectId,
    pub draft_version: String,
    pub created_at: DateTime<Utc>,
}

impl crate::contracts::VersionedContract for WorkspaceMetadata {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::WorkspaceMetadata;
}
