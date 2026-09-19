//! Cross-domain read models belong here only when no domain owns the view.

pub mod activity;
pub mod baseline;
pub mod coverage;
pub mod freshness;
pub mod inbox;
pub mod index;
pub mod integrity;
pub mod publication;

pub use activity::{ActivityEntry, ActivityReplay};
pub use baseline::{route_for, RouteRefusal};
pub use coverage::{
    resolve, unproven, CoverageBasis, CoverageInputs, DeclaredCoverage, ProducerAttestation,
    ResourceCoverage,
};
pub use freshness::{
    check, ActionOutcome, Projection, ReadModelWatermark, RequestPrecondition, StaleReason,
    StoreKey,
};
pub use integrity::{verify_all, LedgerVerification};
