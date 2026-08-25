//! Draft's canonical domain model and orchestration APIs.
//!
//! Product version 0.3.4 ships independently registered contracts whose
//! current/supported policies are all v1. Public APIs live under their owning
//! domain namespace.

pub mod app;
pub mod contracts;
pub mod operation;
pub mod pack;
pub mod read_model;
pub mod review;
pub mod support;
pub mod task;
pub mod trust;
pub mod workspace;

/// Product/package version. Contract compatibility is declared independently
/// by the typed metadata in [`contracts`].
pub const DRAFT_VERSION: &str = "0.3.4";
/// Product API SemVer used only for extension `draft_api` requirements.
pub const DRAFT_API_VERSION: &str = DRAFT_VERSION;
