//! Actor identity model (who/what performed Draft actions).

pub mod actor;
pub mod global;
pub mod local;

pub use actor::{ActorKind, ActorRef};
pub use global::{ActorProfile, CandidateKind, CandidateRecord, IdentityStatus, PublicKeyRecord};
pub use local::{resolve_actor, save_workspace_identity, IdentityRecord};
