//! The Draft Change Graph: what a project contains, and how it is observed.
//!
//! Resources and their state, the observations that establish it, the sources
//! and adapters that produce those observations, and the accepted Baseline they
//! compose into. Where `project` owns the storage a project lives in, `dcg`
//! owns the graph of what is in it.

pub mod accept;
pub mod anchor;
pub mod baseline;
pub mod change_pack;
pub mod change_pack_projection;
pub mod change_pack_store;
pub mod change_set;
pub mod compose;
pub mod decision;
pub mod definition;
pub mod filesystem_provider;
pub mod filesystem_source;
pub mod impact;
pub mod observation;
pub mod observation_lifecycle;
pub mod observation_set;
pub mod observation_store;
pub mod observe;
pub mod representation;
pub mod resource;
pub mod review;
pub mod revision_pack;
pub mod semantics_registry;
pub mod snapshot;
pub mod source;
pub mod source_view;
pub mod state;

pub use change_pack::{ChangePack, ChangePackGuard, ChangePackLifecycle, ChangePackStore};
pub use change_pack_projection::{
    change_pack_lifecycle_of, review_progress_state_of, ReviewProgress,
};
pub use compose::{CurrentProviderRoutability, HistoricalBaselineComposition, NotRoutable};
pub use definition::{ChangePackDefinition, ScopeResolution};
pub use revision_pack::RevisionPack;
pub use semantics_registry::{RegistrationOutcome, SemanticsContractRegistry};
pub use source_view::{CanonicalSourceView, WorkspaceRevision};
