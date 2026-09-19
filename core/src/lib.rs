//! Draft's canonical domain model and orchestration APIs.
//!
//! Product version 0.3.4 ships independently registered contracts whose
//! current/supported policies are all v1. Public APIs live under their owning
//! domain namespace.

pub mod activity;
pub mod app;
pub mod authority;
pub mod contracts;
pub mod dcg;
pub mod evidence;
pub mod execution;
pub mod extension;
pub mod gate;
pub mod installation;
pub mod project;
pub mod promotion;
pub mod provenance;
pub mod publication;
pub mod read_model;
pub mod receipt;
pub mod recovery;
pub mod support;
pub mod task;
pub mod trust;

/// Product/package version. Contract compatibility is declared independently
/// by the typed metadata in [`contracts`].
pub const DRAFT_VERSION: &str = "0.3.4";
/// Product API SemVer used only for extension `draft_api` requirements.
pub const DRAFT_API_VERSION: &str = DRAFT_VERSION;
