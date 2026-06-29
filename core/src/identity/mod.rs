//! Actor identity model (who/what performed Draft actions).

pub mod actor;
pub mod local;
pub mod provider_mapping;

pub use actor::{ActorKind, ActorRef};
pub use local::{resolve_actor, save_workspace_identity, IdentityRecord};
