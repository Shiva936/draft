//! The one way time enters a decision.
//!
//! # Why time is injected rather than read
//!
//! Anything that reads the wall clock directly cannot be re-verified. A gate
//! evaluation, an expiry check or an authority decision that called
//! `Utc::now()` produces a different answer every time it runs, so "was this
//! correctly decided?" becomes unanswerable — the input moved.
//!
//! Injecting the clock makes the moment an explicit input. A historical fact
//! records the instant it was evaluated at, and re-evaluating against that
//! same instant reproduces the same conclusion.
//!
//! # Why the source is recorded alongside the instant
//!
//! An instant alone does not say where it came from. A decision evaluated
//! against a fixed test clock and one evaluated against the system clock are
//! materially different claims, and a verifier that cannot distinguish them
//! would treat a fixture as evidence. So every clock names itself, and
//! `EvaluationContext` carries that name next to `evaluated_at`.

use draft_dcg_contract::identifier::NamespacedId;
use draft_dcg_contract::value::Timestamp;

/// A source of time.
pub trait Clock: Send + Sync {
    /// The current instant, according to this source.
    fn now(&self) -> Timestamp;

    /// What this source is, recorded beside every instant it produces.
    fn source(&self) -> NamespacedId;
}

/// The host's wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_nanos(
            chrono::Utc::now()
                .timestamp_nanos_opt()
                .expect("system time is representable as nanoseconds since the epoch"),
        )
    }

    fn source(&self) -> NamespacedId {
        NamespacedId::parse("draft.core/system-clock").expect("a frozen literal is valid")
    }
}

/// A clock that does not move.
///
/// For tests and for re-evaluating a historical decision at the instant it was
/// originally decided. It names itself distinctly so a fact evaluated against
/// it can never be mistaken for one evaluated against real time.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(Timestamp);

impl FixedClock {
    pub fn at(instant: Timestamp) -> Self {
        Self(instant)
    }

    pub fn at_unix_nanos(nanos: i64) -> Self {
        Self(Timestamp::from_unix_nanos(nanos))
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.0
    }

    fn source(&self) -> NamespacedId {
        NamespacedId::parse("draft.core/fixed-clock").expect("a frozen literal is valid")
    }
}

/// The moment a decision was made, and where that moment came from.
///
/// Carried by every evaluated fact so the decision can be reproduced.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluatedAt {
    pub instant: Timestamp,
    pub clock_source: NamespacedId,
}

impl EvaluatedAt {
    /// Read the moment from a clock, capturing both halves together.
    ///
    /// Taking them as one operation is deliberate: an instant from one clock
    /// recorded beside another clock's name would be a fact that misdescribes
    /// its own provenance.
    pub fn now(clock: &dyn Clock) -> Self {
        Self {
            instant: clock.now(),
            clock_source: clock.source(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_clock_does_not_move() {
        let clock = FixedClock::at_unix_nanos(1_000);
        assert_eq!(clock.now(), clock.now());
        assert_eq!(clock.now(), Timestamp::from_unix_nanos(1_000));
    }

    #[test]
    fn a_fixed_evaluation_cannot_be_mistaken_for_a_real_one() {
        // A verifier must be able to tell a fixture from the system clock. If
        // both reported the same source, a decision made against a frozen
        // instant would read as evidence about the real world.
        let fixed = EvaluatedAt::now(&FixedClock::at_unix_nanos(0));
        let system = EvaluatedAt::now(&SystemClock);
        assert_ne!(fixed.clock_source, system.clock_source);
    }

    #[test]
    fn the_system_clock_advances() {
        let clock = SystemClock;
        let first = clock.now();
        let second = clock.now();
        assert!(second.as_unix_nanos() >= first.as_unix_nanos());
    }
}
