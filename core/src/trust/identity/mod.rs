//! Actor identity model (who/what performed Draft actions).

pub mod global;
pub mod local;

pub use global::{ActorProfile, CandidateKind, CandidateRecord, IdentityStatus, PublicKeyRecord};
pub use local::{reject_retired_profile_state, resolve_actor};
