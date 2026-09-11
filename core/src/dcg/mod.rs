//! The Draft Change Graph: what a project contains, and how it is observed.
//!
//! Resources and their state, the observations that establish it, the sources
//! and adapters that produce those observations, and the accepted Baseline they
//! compose into. Where `project` owns the storage a project lives in, `dcg`
//! owns the graph of what is in it.

pub mod accept;
pub mod anchor;
pub mod baseline;
pub mod change;
pub mod change_projection;
pub mod change_set;
pub mod change_store;
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
pub mod revision;
pub mod semantics_registry;
pub mod snapshot;
pub mod source;
pub mod source_view;
pub mod state;

pub use change::{Change, ChangeGuard, ChangeLifecycle, ChangeStore};
pub use change_projection::{change_lifecycle_of, revision_state_of, ReviewProgress};
pub use compose::{CurrentProviderRoutability, HistoricalBaselineComposition, NotRoutable};
pub use definition::{ChangeDefinition, ScopeResolution};
pub use revision::ChangeRevision;
pub use semantics_registry::{RegistrationOutcome, SemanticsContractRegistry};
pub use source_view::{CanonicalSourceView, WorkspaceRevision};
