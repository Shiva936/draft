//! Interrupting the promotion protocol at each durable boundary.
//!
//! The restart table is tested as a function elsewhere. These drive it through
//! the real engine: each test leaves the durable records in the state a crash
//! at that exact point would leave them, then runs the protocol again and
//! checks what it concludes.
//!
//! The point of every one is the same — a promotion interrupted mid-protocol
//! must never accept the same work twice, and must never leave the project
//! unable to say whether it accepted it at all.

use std::cell::RefCell;

use draft_core::promotion::journal::{ChangePackMatch, PromotionJournalState};
use draft_core::promotion::protocol::{
    execute, require_coverage, PromotionEffects, PromotionProgress, PromotionStores,
};
use draft_core::promotion::record::PromotionJournal;
use draft_core::support::error::{DraftError, DraftErrorKind, DraftResult};
use draft_dcg_contract::coverage::{CoverageDomainRef, CoverageEvidence, CoverageStatus};
use draft_dcg_contract::ids::{
    ActorId, ChangePackId, PromotionId, ProviderBindingId, ReceiptId, RevisionPackId,
};
use draft_dcg_contract::receipt::ReceiptSignerBinding;
use draft_dcg_contract::{BaselineId, Digest, ProviderSemanticDefinitionDigest};

/// A stand-in for the application's durable steps.
///
/// Records what the protocol asked for and lets a test stop partway, which is
/// how a crash at a chosen boundary is reproduced without killing a process.
struct Recorder {
    control: RefCell<Digest>,
    committed: RefCell<Vec<BaselineId>>,
    completed: RefCell<usize>,
    finalized: RefCell<usize>,
    /// Fail the commit, as an interrupted process would.
    fail_commit: bool,
    /// Fail after the Baseline exists but before finalization completes.
    fail_finalize: bool,
}

impl Recorder {
    fn new(control: Digest) -> Self {
        Self {
            control: RefCell::new(control),
            committed: RefCell::new(Vec::new()),
            completed: RefCell::new(0),
            finalized: RefCell::new(0),
            fail_commit: false,
            fail_finalize: false,
        }
    }

    fn failing_commit(control: Digest) -> Self {
        Self {
            fail_commit: true,
            ..Self::new(control)
        }
    }

    fn failing_finalize(control: Digest) -> Self {
        Self {
            fail_finalize: true,
            ..Self::new(control)
        }
    }

    fn commits(&self) -> usize {
        self.committed.borrow().len()
    }
}

impl PromotionEffects for Recorder {
    fn control_digest(&self) -> DraftResult<Digest> {
        Ok(self.control.borrow().clone())
    }

    fn change_match(&self, _journal: &PromotionJournal) -> DraftResult<ChangePackMatch> {
        Ok(ChangePackMatch::ExpectedActive)
    }

    fn commit_baseline(&self, journal: &PromotionJournal) -> DraftResult<BaselineId> {
        if self.fail_commit {
            return Err(DraftError::new(
                DraftErrorKind::Storage,
                "the process died while accepting the Baseline",
            ));
        }
        let baseline = BaselineId::new(Digest::of_bytes(
            format!("baseline|{}", journal.promotion).as_bytes(),
        ));
        self.committed.borrow_mut().push(baseline.clone());
        // The control state moves with the acceptance, exactly as the live
        // path's does: this is what recovery compares against.
        *self.control.borrow_mut() = journal.planned_control.clone();
        Ok(baseline)
    }

    fn complete_change(&self, _journal: &PromotionJournal) -> DraftResult<()> {
        *self.completed.borrow_mut() += 1;
        Ok(())
    }

    fn finalize(&self, _journal: &PromotionJournal, _baseline: &BaselineId) -> DraftResult<()> {
        if self.fail_finalize {
            return Err(DraftError::new(
                DraftErrorKind::Storage,
                "the process died while issuing the receipt",
            ));
        }
        *self.finalized.borrow_mut() += 1;
        Ok(())
    }
}

fn promotion() -> PromotionId {
    PromotionId::parse("pro_000000000001").unwrap()
}

fn expected_control() -> Digest {
    Digest::of_bytes(b"control-before")
}

fn intent() -> PromotionJournal {
    PromotionJournal {
        prepared_at: draft_dcg_contract::value::Timestamp::from_unix_nanos(0),
        promotion: promotion(),
        revision_pack: RevisionPackId::parse("rpk_000000000001").unwrap(),
        change_pack: ChangePackId::parse("cpk_000000000001").unwrap(),
        baseline: BaselineId::new(Digest::of_bytes(b"intended")),
        receipt: ReceiptId::parse("rcp_000000000001").unwrap(),
        signer: ReceiptSignerBinding::new(
            ActorId::parse("act_000000000001").unwrap(),
            "key-1",
            "ed25519",
        )
        .unwrap(),
        activity_event_ids: vec!["evt_000000000001".into()],
        expected_control: expected_control(),
        planned_control: Digest::of_bytes(b"control-after"),
        planned_change: Digest::of_bytes(b"change-completed"),
        state: PromotionJournalState::Prepared,
    }
}

fn stores(directory: &tempfile::TempDir) -> PromotionStores {
    PromotionStores {
        journals: draft_core::promotion::store::PromotionJournalStore::new(
            directory.path().join("journal"),
        ),
        records: draft_core::promotion::record::PromotionRecordStore::new(
            directory.path().join("records"),
        ),
    }
}

#[test]
fn an_uninterrupted_promotion_commits_once_and_finalizes() {
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    let effects = Recorder::new(expected_control());

    let progress = execute(&stores, &intent(), &effects).unwrap();
    assert!(matches!(progress, PromotionProgress::Promoted { .. }));
    assert_eq!(effects.commits(), 1);
    assert_eq!(*effects.finalized.borrow(), 1);

    let journal = stores
        .journals
        .read_unlocked(&promotion())
        .unwrap()
        .unwrap();
    assert_eq!(journal.state(), PromotionJournalState::Finalized);
}

#[test]
fn an_interruption_before_any_durable_work_leaves_nothing_behind() {
    // The journal is written before the commit, so a failure to even open it
    // must leave no promotion at all — not a half-started one.
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    let mut degenerate = intent();
    degenerate.activity_event_ids.clear();

    assert!(execute(&stores, &degenerate, &Recorder::new(expected_control())).is_err());
    assert!(stores
        .journals
        .read_unlocked(&promotion())
        .unwrap()
        .is_none());
}

#[test]
fn an_interruption_at_the_commit_leaves_a_prepared_journal_that_retries() {
    // The crash window the write order exists for: the intent is durable and
    // nothing was accepted. Recovery reads "did not commit" from the control
    // state and runs the promotion, rather than assuming either way.
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);

    let crashed = Recorder::failing_commit(expected_control());
    assert!(execute(&stores, &intent(), &crashed).is_err());
    assert_eq!(crashed.commits(), 0);

    let journal = stores
        .journals
        .read_unlocked(&promotion())
        .unwrap()
        .unwrap();
    assert_eq!(journal.state(), PromotionJournalState::Prepared);

    // The retry goes through the restart table and completes the promotion.
    let retry = Recorder::new(expected_control());
    let progress = execute(&stores, &intent(), &retry).unwrap();
    assert!(matches!(progress, PromotionProgress::Promoted { .. }));
    assert_eq!(
        retry.commits(),
        1,
        "the retry accepted exactly one Baseline"
    );
}

#[test]
fn an_interruption_after_the_baseline_commits_never_accepts_it_twice() {
    // The dangerous window: the project has already accepted the work, and a
    // naive retry would accept it again under a second Baseline.
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);

    let crashed = Recorder::failing_finalize(expected_control());
    assert!(execute(&stores, &intent(), &crashed).is_err());
    assert_eq!(crashed.commits(), 1, "the Baseline was accepted");

    let journal = stores
        .journals
        .read_unlocked(&promotion())
        .unwrap()
        .unwrap();
    assert_eq!(
        journal.state(),
        PromotionJournalState::Committed,
        "the journal records the commit even though finalization did not finish"
    );

    // Restart: the control state now matches the journal's *planned* value, so
    // the restart table concludes the promotion committed and only owes its
    // finalization.
    let retry = Recorder::new(journal.journal.planned_control.clone());
    let progress = execute(&stores, &intent(), &retry).unwrap();

    assert!(matches!(
        progress,
        PromotionProgress::ResumedAndFinalized { .. }
    ));
    assert_eq!(
        retry.commits(),
        0,
        "a committed promotion must never accept a second Baseline"
    );
    assert_eq!(*retry.finalized.borrow(), 1);
}

#[test]
fn restarting_a_finished_promotion_reports_it_as_already_done() {
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    let first = Recorder::new(expected_control());
    let promoted = execute(&stores, &intent(), &first).unwrap();

    // Control has moved on since; a finalized promotion is history and does
    // not require current state to still match anything.
    let later = Recorder::new(Digest::of_bytes(b"control-much-later"));
    let again = execute(&stores, &intent(), &later).unwrap();

    assert!(matches!(again, PromotionProgress::AlreadyFinalized { .. }));
    assert_eq!(again.baseline(), promoted.baseline());
    assert_eq!(later.commits(), 0);
}

#[test]
fn a_control_state_matching_neither_value_is_refused_rather_than_guessed() {
    // The impossible combination: the journal is Prepared and the control
    // state is neither what it expected nor what it planned. Something else
    // moved it, and guessing would either discard an acceptance or invent one.
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);

    let crashed = Recorder::failing_commit(expected_control());
    assert!(execute(&stores, &intent(), &crashed).is_err());

    let confused = Recorder::new(Digest::of_bytes(b"somebody-elses-control-state"));
    let error = execute(&stores, &intent(), &confused).unwrap_err();
    assert_eq!(error.kind, DraftErrorKind::CorruptData);
    assert_eq!(confused.commits(), 0);
}

#[test]
fn a_second_promotion_is_barred_while_an_earlier_one_owes_its_completion() {
    // The barrier. An earlier promotion committed but never completed its
    // ChangePack; starting a new one over the top would let the same work be
    // accepted twice.
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);

    let crashed = Recorder::failing_finalize(expected_control());
    assert!(execute(&stores, &intent(), &crashed).is_err());

    // A different promotion, while the first is Committed and its ChangePack is
    // still the state the journal expected.
    let mut second = intent();
    second.promotion = PromotionId::parse("pro_000000000002").unwrap();
    second.expected_control = Digest::of_bytes(b"control-after");
    second.planned_control = Digest::of_bytes(b"control-after-second");

    let blocked = Recorder::new(Digest::of_bytes(b"control-after"));
    let error = execute(&stores, &second, &blocked).unwrap_err();
    assert_eq!(error.kind, DraftErrorKind::ConflictDetected);
    assert!(error
        .message
        .contains("ChangePack completion did not finish"));
    assert_eq!(
        blocked.commits(),
        0,
        "the barrier must stop the authority change, not merely warn"
    );
}

#[test]
fn insufficient_coverage_prevents_a_promotion_from_starting() {
    // Absence has to be proved. A domain nobody enumerated is not an empty
    // domain, and accepting a Baseline on that basis records "this is
    // everything" when the truth is "this is what we could see".
    let binding = ProviderBindingId::parse("pbd_000000000001").unwrap();
    let semantics = ProviderSemanticDefinitionDigest::new(Digest::of_bytes(b"SD1"));
    let covered = CoverageEvidence {
        provider_binding: binding.clone(),
        provider_semantic_definition: semantics.clone(),
        domain: CoverageDomainRef::parse("files").unwrap(),
        status: CoverageStatus::Complete,
        observation_run: None,
        attempted: true,
        committed: true,
        known_gaps: Default::default(),
    };

    let required: std::collections::BTreeSet<String> = ["files".to_string(), "records".to_string()]
        .into_iter()
        .collect();
    let error = require_coverage(std::slice::from_ref(&covered), &required).unwrap_err();
    assert_eq!(error.kind, DraftErrorKind::CoverageIncomplete);
    assert!(error.message.contains("records"));

    // With the missing domain covered, the same check passes.
    let records = CoverageEvidence {
        domain: CoverageDomainRef::parse("records").unwrap(),
        ..covered.clone()
    };
    require_coverage(&[covered, records], &required).unwrap();
}
