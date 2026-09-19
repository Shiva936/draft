//! Who may do what, and the evidence for it.
//!
//! Three things live here, deliberately separated:
//!
//! | | Answers | Mutable? |
//! |---|---|---|
//! | [`grant::AuthorityGrant`] | someone was granted a capability | never |
//! | [`revocation::AuthorityRevocation`] | a grant was withdrawn | never |
//! | [`evaluation`] | may this claim proceed *now* | recomputed every time |
//!
//! # Why revocation is a fact rather than a deletion
//!
//! Deleting a grant would erase the evidence that it once authorized
//! something. Every receipt, decision and audit fact that cited it would then
//! reference material nobody can find, and "this was authorized at the time"
//! would become indistinguishable from "this was never authorized".
//!
//! So a revocation is its own immutable fact naming the grant it withdraws.
//! Both survive; evaluation reads both and concludes.
//!
//! # Why evaluation is not cached
//!
//! An [`AuthorityDecision`] is history: it records that a conclusion was
//! reached against named facts at a named moment. It is never a licence to
//! act again later. Current authority is re-evaluated at the commit boundary
//! of the *next* operation, under the locks that boundary requires — because
//! a grant that was live when the plan was made may have been revoked since,
//! and the whole point of a revocation is that it takes effect.
//!
//! [`AuthorityDecision`]: draft_dcg_contract::authority::AuthorityDecision

pub mod evaluation;
pub mod grant;
pub mod revocation;

pub use evaluation::{evaluate, AuthorityClaim, AuthorityInputs};
pub use grant::{AuthorityGrant, AuthorityGrantStore};
pub use revocation::{AuthorityRevocation, AuthorityRevocationStore};
