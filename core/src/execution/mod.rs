//! Execution: running work against a project under an authorized route.
//!
//! Operations, the processes and records that carry them out, leases and their
//! fencing, and the mechanisms that propose and apply changes. Where `dcg`
//! describes what a project contains, `execution` is what actually does
//! something to it — which is why an exact route is validated here, under the
//! binding's lock, before any external side effect.

pub mod command_adapter;
pub mod lease;
pub mod mechanism;
pub mod notification;
pub mod operation;
pub mod plan;
pub mod process;
pub mod records;
pub mod workspace;
