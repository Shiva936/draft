//! Recovery: proving what a past state was, and restoring it.
//!
//! An anchor is continuity evidence captured under the same fencing that
//! produced the state it describes. Capturing one later would be evidence about
//! a different moment with no way to tell, so where fencing fails no anchor is
//! written — a missing anchor is honest, a mismatched one is not.

pub mod plan;
pub mod rollback;

pub use plan::{RecoveryAction, RecoveryPlan, RecoveryPlanStore};
