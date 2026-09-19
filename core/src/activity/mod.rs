//! The Activity Ledger: an append-only record of what actually happened.
//!
//! Activity is history, not intent. An event is appended when the fact it
//! describes is already durable, which is why the vocabulary avoids names that
//! could outlive the thing they claim — a dispatch that Draft *authorized* is
//! not a request anything external *received*, and only one of those can
//! honestly be recorded before the call is made.
//!
//! The ledger is:
//!
//! * **serialized** by a correctness lock on `events/events.lock`, so two
//!   appenders cannot both read the same tail hash and fork the chain;
//! * **framed**, so a crash mid-append is distinguishable from damage to a
//!   record that was committed (see [`frame`]);
//! * **payload-idempotent**, so replaying an undrained audit fact appends
//!   nothing the second time;
//! * **rebuildable** — `events/events.index` is derived, and
//!   `events/events.log` is the only authoritative file.

pub mod event;
pub mod frame;
pub mod global;
pub mod log;

pub use event::{AuditFactKind, EventKind, EventOwnership, JournalMechanism};
pub use frame::{FrameScan, MAX_RECORD_LENGTH};
pub use global::{GlobalAuditEntry, GlobalAuditEvent, GlobalAuditLog};
pub use log::{ActivityLog, ActivityReader, AppendOutcome, LedgerRecord, RecoveryReport};
