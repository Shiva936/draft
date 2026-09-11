//! Draft extension infrastructure: sources, catalog trust, discovery,
//! acquisition, validation, installation, installed state, authorization and
//! provenance.
//!
//! Extension *semantics* — identity, manifests, contributions, compatibility,
//! permissions and lifecycle — belong to Draft Core. This crate owns only the
//! infrastructure that acquires packages, durably records what is installed,
//! and remembers which capabilities the user authorized for which artifact.

pub mod authorization;
pub mod catalog;
pub mod contributions;
pub mod discovery;
pub mod extension;
pub mod official;
