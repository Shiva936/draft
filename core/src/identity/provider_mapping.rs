//! Optional mapping between Draft actors and provider-native identities.
//!
//! v0.2.0 keeps this minimal: providers own their own committer identity (e.g.
//! Git uses `user.name`/`user.email`), while Draft tracks *who ran Draft*. This
//! module exists as the extension point for richer mapping later.

use super::actor::ActorRef;

/// A co-author trailer line derived from an actor, for finalization messages.
pub fn coauthor_trailer(actor: &ActorRef, email: &str) -> String {
    format!("Co-authored-by: {} <{}>", actor.display_name, email)
}
