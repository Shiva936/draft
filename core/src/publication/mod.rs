//! Publication: delivering an accepted Baseline to an external system.
//!
//! This is the only part of Draft that causes effects outside Draft, and that
//! one fact shapes everything here. A local mutation that half-happened can be
//! rolled back; an external mutation that half-happened cannot. So Publication
//! is built to answer one question honestly — *did the external mutation
//! occur?* — and to refuse rather than guess when it cannot.
//!
//! # The shape of a dispatch
//!
//! ```text
//! Phase 0  local bookkeeping barrier     ends holding NOTHING, yields a result
//! Phase 1  current authority and route   fence → lease → control → binding
//! Phase 2  allocation                    … → journal → publication control
//! Phase 3  durable Dispatching           the same guards, then release ALL
//!          ─────────────────────────────────────────────────────────────────
//!          only now is the external system called
//! ```
//!
//! Two independent transactions. Phase 0 is local-first and ends holding
//! nothing at all, so Phase 1 can take the trust fence first without a reverse
//! acquisition. Phases 1–3 hold their guards continuously through the durable
//! `Dispatching` boundary, so nothing they validated can move before the
//! dispatch is committed — and then release every one of them before the call
//! is made.
//!
//! **No lock is held across the external call.** Not the attempt journal, not
//! the binding, not the control record, not a lease. An unreachable external
//! system must never turn into "Draft will not let me edit my own work".
//!
//! # What each module owns
//!
//! * [`control`] — the correctness authority for one Publication: whether an
//!   attempt is in flight, which numbers are spent, which one-shot
//!   authorizations are consumed. Allocation and its checks are atomic
//!   because they are reachable only through a live guard.
//! * [`journal`] — the per-attempt state machine, and the only way to change
//!   it. Several actors legitimately touch one attempt, so there is no
//!   unguarded read-and-overwrite anywhere.
//! * [`outcome`] — the at-most-one primary outcome, written object-before-head
//!   so the only reachable crash state is a collectable orphan.
//! * [`completion`] — the single path every primary-outcome writer takes, and
//!   the convergence rule a worker returning from the external call obeys.
//! * [`restart`] — the three-dimensional restart table: journal × control ×
//!   outcome, as one total function.
//! * [`barrier`] — Phase 0, whose `Clean` result is the only gate to a new
//!   attempt.
//! * [`authority`] — Phase 1: whose authority the effect happens under, and
//!   the guards that stay held until the dispatch boundary is durable.
//! * [`consistency`] — the cross-record half of §2.46: the checks a digest
//!   cannot make because they need a second durable record.
//! * [`delivery`] — what each delivery class lets recovery conclude, and
//!   separately whether it may retry.
//! * [`recovery`] — the phase discipline: what must be re-read at every
//!   boundary, because no guard survives one.
//! * [`abandon`] — withdrawing an allocated attempt whose dispatch is refused
//!   before it happens, which is the only interruption Draft can resolve
//!   without asking anybody.
//! * [`registry`] — creation: one request key, one Publication, forever.
//! * [`retry`] — the acknowledged decision that lets an uncertain delivery be
//!   attempted again.
//! * [`store`] — where that creation rule meets durable storage.
//! * [`resolution`] — the Phase R → Phase C shape shared by Resolution and
//!   retry-authorization creation, where recovery never needs current
//!   authority and creation never skips it.
//!
//! # The distinctions that are never collapsed
//!
//! * **A candidate number is not an authoritative one.** Only a committed
//!   allocation consumes a number; an interrupted worker burns nothing.
//! * **`Abandoned` is not `AbandonedBeforeDispatch`.** One says the allocation
//!   never committed, the other says it did and dispatch was then refused —
//!   opposite answers about what was spent.
//! * **A staged attempt is not a dispatched one.** A journal record existing is
//!   never proof that a request was made.
//! * **A late external result is not a candidate outcome.** It is operational
//!   input; only an authorized Resolution makes an interpretation
//!   authoritative.
//! * **A `record_once` conflict is not concurrency.** Candidate selection is
//!   serialized, so a conflicting outcome implies a bypass, and is never
//!   resolved by converging on one.

pub mod abandon;
pub mod authority;
pub mod barrier;
pub mod completion;
pub mod consistency;
pub mod control;
pub mod delivery;
pub mod dispatch;
pub mod journal;
pub mod outcome;
pub mod recovery;
pub mod registry;
pub mod resolution;
pub mod resolve;
pub mod restart;
pub mod retry;
pub mod store;

pub use abandon::{classify as classify_for_withdrawal, withdraw, Withdrawn};
pub use authority::{under_authority, AuthorityRequest, AuthorityStores, DispatchAuthority};
pub use barrier::{BarrierInputs, PublicationBookkeepingResult};
pub use completion::{converge, prepare_and_record_primary_outcome, ReturningWorker};
pub use consistency::{retry_authorization_targets, verify_dispatched_attempt};
pub use control::{
    ControlMatch, PlannedAllocation, PublicationControl, PublicationControlGuard,
    PublicationControlStore,
};
pub use delivery::{recovery_class, retry_permission, RetryPermission};
pub use dispatch::{
    bookkeeping, dispatch, DeliveryResult, DispatchProgress, DispatchRequest, DispatchStores,
};
pub use journal::{
    AttemptJournal, AttemptJournalGuard, AttemptJournalState, ControlTransition, NonCommitEvidence,
    PublicationJournalStore, TerminalDisposition,
};
pub use outcome::{PrimaryOutcomeIdentity, PublicationOutcomeStore, RecordOnce};
pub use recovery::{
    recovery_may_allocate, withdrawal_path, BoundaryCheck, FreshValidation, PhaseBoundary,
    WithdrawalPath,
};
pub use registry::{classify_creation, CreationJournalState, CreationOutcome, MappedPublication};
pub use resolution::{
    classify_prior, creation_step, CreationStep, FactCreationState, HeadMatch, PriorTransaction,
};
pub use resolve::{resolve, ResolutionStore, ResolveRequest};
pub use restart::{AttemptResolution, AttemptSnapshot, DispatchRecoveryClass, OutcomePresence};
pub use retry::{authorize, require_targets, RetryAuthorizationStore, RetryRequest};
pub use store::{id_for_request_key, PublicationAttemptStore, PublicationStore};
