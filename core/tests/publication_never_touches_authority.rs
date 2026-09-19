//! Publication through its real dispatch engine, and what it may never do.
//!
//! The engine's pieces are tested individually elsewhere. These drive the
//! whole dispatch — Phase 1 authority, the barrier, allocation, the durable
//! `Dispatching` boundary, the external call, the primary outcome, the control
//! clear — and check the one property that makes publication safe to attempt
//! at all: however it ends, the project still accepts exactly what it accepted
//! before.
//!
//! Everything goes through `app::publish`, because that is the only way the
//! engine is correctly reachable. A `DispatchRequest` assembled by hand could
//! name a route the binding does not select, an authority nobody granted, or
//! an attempt against a Publication that does not exist — configurations Draft
//! cannot produce, so proving something about them proves nothing.

mod support;

use draft_core::app::publish::{publish, PublishOutcome};
use draft_core::publication::dispatch::{bookkeeping, DeliveryResult, DispatchStores};
use draft_core::publication::PublicationBookkeepingResult;
use draft_dcg_contract::publication::{DeliverySemantics, PublicationOutcomeKind};
use support::publishing::PublishingProject;

/// Dispatch once, with the delivery this test wants to inject.
fn deliver(
    project: &PublishingProject,
    request_id: &str,
    semantics: DeliverySemantics,
    result: DeliveryResult,
) -> PublishOutcome {
    let request = project.request(request_id, semantics);
    publish(&project.workspace, &request, || result).unwrap()
}

/// What the engine's own barrier says about this Publication right now.
fn barrier(
    project: &PublishingProject,
    semantics: DeliverySemantics,
) -> PublicationBookkeepingResult {
    let request = project.request("barrier-probe", semantics);
    let publication = project.publication(&request);
    bookkeeping(
        &DispatchStores::for_layout(&project.workspace.layout),
        &publication,
        semantics,
    )
    .unwrap()
}

/// The accepted Baseline, which nothing below may move.
fn accepted(project: &PublishingProject) -> draft_dcg_contract::BaselineId {
    draft_core::dcg::baseline::current_baseline(&project.workspace.layout)
        .unwrap()
        .unwrap()
}

#[test]
fn a_successful_dispatch_records_its_outcome_and_clears_the_control() {
    let project = PublishingProject::new();
    let before = accepted(&project);

    let outcome = deliver(
        &project,
        "success",
        DeliverySemantics::IdempotentByKey,
        DeliveryResult::Succeeded {
            external_reference: "remote-1".into(),
        },
    );
    assert!(matches!(
        outcome,
        PublishOutcome::Concluded {
            outcome: PublicationOutcomeKind::Succeeded { .. },
            ..
        }
    ));

    // The Publication is idle again, so it may be attempted afresh; and the
    // project still accepts exactly what it accepted before.
    assert_eq!(
        barrier(&project, DeliverySemantics::IdempotentByKey),
        PublicationBookkeepingResult::Clean
    );
    assert_eq!(accepted(&project), before);
}

#[test]
fn a_failed_delivery_still_records_an_outcome_and_frees_the_publication() {
    // The external system refusing is a real answer, not an absence of one.
    // It must be recorded, or a later attempt could not tell "we tried and it
    // said no" from "we never tried".
    let project = PublishingProject::new();
    let before = accepted(&project);

    let outcome = deliver(
        &project,
        "refused",
        DeliverySemantics::IdempotentByKey,
        DeliveryResult::Failed {
            reason: "the target rejected it".into(),
        },
    );
    assert!(matches!(
        outcome,
        PublishOutcome::Concluded {
            outcome: PublicationOutcomeKind::Failed { .. },
            ..
        }
    ));
    assert_eq!(
        barrier(&project, DeliverySemantics::IdempotentByKey),
        PublicationBookkeepingResult::Clean
    );
    assert_eq!(
        accepted(&project),
        before,
        "a refused delivery has no authority over what the project accepts"
    );
}

#[test]
fn an_undetermined_delivery_is_recorded_as_indeterminate_not_as_failure() {
    // Retrying a refusal re-attempts something that did not happen; retrying
    // this may duplicate something that did. The engine must keep them apart.
    let project = PublishingProject::new();

    let outcome = deliver(
        &project,
        "undetermined",
        DeliverySemantics::IdempotentByKey,
        DeliveryResult::Undetermined {
            reason: "the connection dropped before the reply".into(),
        },
    );
    assert!(matches!(
        outcome,
        PublishOutcome::Concluded {
            outcome: PublicationOutcomeKind::Indeterminate { .. },
            ..
        }
    ));
}

#[test]
fn an_interrupted_dispatch_holds_the_publication_until_it_is_resumed() {
    // The crash window that matters: the attempt reached the durable
    // Dispatching boundary and the process died before any outcome. Draft
    // cannot prove whether the external effect occurred, so a second attempt
    // must not begin.
    let project = PublishingProject::new();
    let before = accepted(&project);

    let interrupted = project.request("interrupted", DeliverySemantics::IdempotentByKey);
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        publish(&project.workspace, &interrupted, || {
            panic!("the process died mid-delivery")
        })
    }));
    assert!(crashed.is_err());

    // A genuinely new send is refused: the barrier sends the caller back to
    // the attempt nobody can describe yet.
    let blocked = publish(
        &project.workspace,
        &project.request("a-new-send", DeliverySemantics::IdempotentByKey),
        || panic!("a second external effect must never be attempted"),
    )
    .unwrap();
    assert!(
        matches!(blocked, PublishOutcome::ResumeRequired { .. }),
        "expected the barrier to refuse: {blocked:?}"
    );
    assert_eq!(accepted(&project), before);
}

#[test]
fn an_unresolvable_attempt_awaits_external_resolution_without_blocking_locally() {
    // Delivery semantics that can only learn the truth by asking. Nothing
    // local waits on the network; this Publication simply may not dispatch
    // again until the effect is resolved.
    let project = PublishingProject::new();

    let interrupted = project.request("unresolvable", DeliverySemantics::QueryByClientKey);
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        publish(&project.workspace, &interrupted, || {
            panic!("the process died mid-delivery")
        })
    }));
    assert!(crashed.is_err());

    assert!(
        matches!(
            barrier(&project, DeliverySemantics::QueryByClientKey),
            PublicationBookkeepingResult::PendingExternalResolution { .. }
        ),
        "a target that can only be asked yields a pending publication"
    );
}

#[test]
fn re_dispatching_a_concluded_publication_converges_on_the_recorded_outcome() {
    // A retry after an uncertain interruption must not produce a second
    // external effect, and must not re-compete the recorded answer.
    let project = PublishingProject::new();
    let request = project.request("converge", DeliverySemantics::IdempotentByKey);

    let first = publish(&project.workspace, &request, || DeliveryResult::Succeeded {
        external_reference: "remote-1".into(),
    })
    .unwrap();
    assert!(first.is_delivered());

    let again = publish(&project.workspace, &request, || {
        panic!("a concluded attempt must never be delivered again")
    })
    .unwrap();
    assert!(
        matches!(again, PublishOutcome::AlreadyConcluded { .. }),
        "expected convergence, got {again:?}"
    );
    assert_eq!(first.attempt(), again.attempt());
}

#[test]
fn every_delivery_class_gets_the_recovery_its_semantics_imply() {
    // The four classes, each interrupted at the same point, each classified by
    // what its delivery guarantees rather than by anything a caller said.
    //
    //   IdempotentByKey       re-sending cannot duplicate → resolvable here
    //   ReconcileByClientKey  the truth is knowable by asking → pending
    //   QueryByClientKey      the truth is knowable by asking → pending
    //   NonIdempotent         nothing to ask, nothing to repeat → resolvable
    //
    // `NonIdempotent` landing in the same bucket as `IdempotentByKey` is the
    // pair worth checking: Draft can close the attempt locally either way, and
    // only one of them may then be sent again.
    let expectations = [
        (
            DeliverySemantics::IdempotentByKey,
            false,
            "re-sending under the same key cannot duplicate, so nothing needs asking",
        ),
        (
            DeliverySemantics::ReconcileByClientKey,
            true,
            "the provider can reconcile by our key, but only if we ask it",
        ),
        (
            DeliverySemantics::QueryByClientKey,
            true,
            "the provider can be queried, but only if we ask it",
        ),
        (
            DeliverySemantics::NonIdempotent,
            false,
            "nothing can be asked, so the attempt closes on local facts alone",
        ),
    ];

    for (semantics, needs_external, why) in expectations {
        let project = PublishingProject::new();
        let interrupted = project.request("class-probe", semantics);
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            publish(&project.workspace, &interrupted, || {
                panic!("the process died mid-delivery")
            })
        }));
        assert!(crashed.is_err());

        let observed = barrier(&project, semantics);
        let pending = matches!(
            observed,
            PublicationBookkeepingResult::PendingExternalResolution { .. }
        );
        assert_eq!(
            pending, needs_external,
            "{semantics:?}: {why} (got {observed:?})"
        );
    }
}

#[test]
fn a_caller_cannot_ask_for_a_recovery_class_its_semantics_do_not_allow() {
    // The property is structural: a publish request carries the delivery
    // semantics and no recovery class, so the only way to reach
    // `ResolvableLocally` for a query-based target would be to change what the
    // target guarantees.
    //
    // Asserted behaviourally by asking the same interrupted attempt under both
    // classifications. If a caller could pick, these two would agree.
    let project = PublishingProject::new();
    let interrupted = project.request("class-conflict", DeliverySemantics::QueryByClientKey);
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        publish(&project.workspace, &interrupted, || {
            panic!("the process died mid-delivery")
        })
    }));
    assert!(crashed.is_err());

    assert!(
        matches!(
            barrier(&project, DeliverySemantics::QueryByClientKey),
            PublicationBookkeepingResult::PendingExternalResolution { .. }
        ),
        "a target that can only be asked yields a pending publication"
    );
    assert!(
        matches!(
            barrier(&project, DeliverySemantics::IdempotentByKey),
            PublicationBookkeepingResult::RecoverAllocatedAttempt { .. }
        ),
        "and a target that is safe to re-send yields a locally recoverable one; the answer \
         follows from the semantics, so nothing above this can choose between them"
    );
}

#[test]
fn an_attempt_is_verified_against_its_publication_and_journal_before_it_concludes() {
    // §2.46's cross-record half, on the live path. The attempt object's own
    // digest proves its bytes have not changed and says nothing about whether
    // it agrees with the Publication it claims or the journal that dispatched
    // it — so a stored attempt whose bytes no longer match the reference the
    // journal recorded must be refused rather than concluded from.
    let project = PublishingProject::new();
    let request = project.request("tampered", DeliverySemantics::IdempotentByKey);

    // Interrupt after the dispatch boundary is durable, so an attempt object
    // and a journal naming it both exist.
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        publish(&project.workspace, &request, || {
            panic!("the process died mid-delivery")
        })
    }));
    assert!(crashed.is_err());

    // Substitute the stored attempt's bytes behind its id.
    let directory = project
        .workspace
        .layout
        .draft_dir
        .join("publication/attempts");
    let stored = std::fs::read_dir(&directory)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|value| value == "json"))
        .expect("the interrupted attempt was stored");
    let mut attempt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&stored).unwrap()).unwrap();
    attempt["attempt_number"] = serde_json::json!(99);
    std::fs::write(&stored, serde_json::to_vec(&attempt).unwrap()).unwrap();

    // Resuming must refuse rather than conclude from bytes nothing vouches for.
    let resumed = publish(&project.workspace, &request, || {
        panic!("a substituted attempt must never be delivered")
    });
    match resumed {
        Ok(PublishOutcome::Inconsistent { .. }) => {}
        Err(error) => assert_eq!(
            error.kind,
            draft_core::support::error::DraftErrorKind::CorruptData,
            "an unexpected error kind: {error:?}"
        ),
        other => panic!("a substituted attempt was accepted: {other:?}"),
    }
}

#[test]
fn a_non_idempotent_delivery_that_ends_uncertain_is_stuck_until_somebody_accepts_the_risk() {
    // The state the retry authorization exists for. Draft asked, could not
    // establish whether the effect occurred, and the target's semantics cannot
    // rule out that re-sending would duplicate it. Nothing Draft can read
    // locally settles that, so it stops — and only a person can restart it.
    let project = PublishingProject::new();
    let request = project.request("uncertain", DeliverySemantics::NonIdempotent);

    let concluded = publish(&project.workspace, &request, || {
        DeliveryResult::Undetermined {
            reason: "the connection dropped before the reply".into(),
        }
    })
    .unwrap();
    assert!(matches!(
        concluded,
        PublishOutcome::Concluded {
            outcome: PublicationOutcomeKind::Indeterminate { .. },
            ..
        }
    ));

    // The engine says another attempt is not safe on the semantics alone.
    assert_eq!(
        draft_core::publication::retry_permission(DeliverySemantics::NonIdempotent),
        draft_core::publication::RetryPermission::RequiresExplicitAuthority
    );

    // Authorizing it is a recorded decision, not a flag.
    let digest = draft_core::app::publish::authorize_retry(
        &project.workspace,
        &request,
        "the target's operator confirmed nothing landed",
    )
    .unwrap();

    let store =
        draft_core::publication::RetryAuthorizationStore::for_layout(&project.workspace.layout);
    let authorization = store.get(&digest).unwrap().expect("it was stored");
    assert!(authorization.duplicate_risk_acknowledged);
    assert!(authorization.authorizes_one_attempt);
    assert!(authorization.authority_decision.is_permitted());
    assert_eq!(
        authorization.rationale,
        "the target's operator confirmed nothing landed"
    );

    // And it permits exactly one further attempt.
    let mut retried = project.request("after-authorization", DeliverySemantics::NonIdempotent);
    retried.retry_authorization = Some(digest.clone());
    let second = publish(&project.workspace, &retried, || DeliveryResult::Succeeded {
        external_reference: "remote-after-retry".into(),
    })
    .unwrap();
    assert!(second.is_delivered(), "got {second:?}");

    // Spending it twice is refused inside the lock that commits the
    // allocation — the one place "has this been spent?" can be answered
    // without a race.
    let mut again = project.request("third", DeliverySemantics::NonIdempotent);
    again.retry_authorization = Some(digest);
    let spent = publish(&project.workspace, &again, || {
        panic!("a consumed authorization must never permit a second attempt")
    });
    match spent {
        Err(error) => assert_eq!(
            error.kind,
            draft_core::support::error::DraftErrorKind::ConflictDetected,
            "got {error:?}"
        ),
        Ok(other) => panic!("a consumed authorization was accepted: {other:?}"),
    }
}

#[test]
fn an_authorization_for_one_publication_cannot_be_spent_on_another() {
    // The check is on the whole `PublicationRef` — id and digest — so bytes
    // changing under `pub_A` cannot quietly widen an existing authorization to
    // a different route, baseline or purpose.
    let project = PublishingProject::new();
    let export = project.request("export", DeliverySemantics::NonIdempotent);
    publish(&project.workspace, &export, || {
        DeliveryResult::Undetermined {
            reason: "unknown".into(),
        }
    })
    .unwrap();

    let digest = draft_core::app::publish::authorize_retry(
        &project.workspace,
        &export,
        "operator confirmed nothing landed",
    )
    .unwrap();

    // A different purpose is a different Publication by construction.
    let mut other = project.request("other-purpose", DeliverySemantics::NonIdempotent);
    other.purpose =
        draft_dcg_contract::publication::PublicationPurposeId::parse("draft.publish/announce")
            .unwrap();
    other.retry_authorization = Some(digest);

    let refused = publish(&project.workspace, &other, || {
        panic!("an authorization bound elsewhere must never permit this")
    });
    match refused {
        Err(error) => assert_eq!(
            error.kind,
            draft_core::support::error::DraftErrorKind::CorruptData,
            "got {error:?}"
        ),
        Ok(outcome) => panic!("an authorization bound elsewhere was accepted: {outcome:?}"),
    }
}

#[test]
fn an_allocated_attempt_that_never_dispatched_can_be_withdrawn_and_the_publication_freed() {
    // The interruption Draft can resolve on its own. The attempt reached its
    // allocation and no further, so the journal proves no external call was
    // made — and without a way to say so, it would block every later attempt
    // forever.
    let project = PublishingProject::new();
    let stores = DispatchStores::for_layout(&project.workspace.layout);
    let request = project.request("stalled", DeliverySemantics::IdempotentByKey);
    let publication = project.publication(&request);

    // Crash between the allocation and the dispatch boundary by making the
    // attempt object unwritable: allocation commits, the boundary never does.
    let attempts = project
        .workspace
        .layout
        .draft_dir
        .join("publication/attempts");
    std::fs::create_dir_all(attempts.parent().unwrap()).unwrap();
    std::fs::write(&attempts, b"not a directory").unwrap();
    let stalled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        publish(&project.workspace, &request, || DeliveryResult::Succeeded {
            external_reference: "must not happen".into(),
        })
    }));
    assert!(
        stalled.is_err() || stalled.as_ref().is_ok_and(|value| value.is_err()),
        "the dispatch should not have completed"
    );
    std::fs::remove_file(&attempts).unwrap();

    // The Publication is held: the barrier refuses a new attempt because it
    // cannot prove the allocated one caused no effect.
    let held = bookkeeping(&stores, &publication, DeliverySemantics::IdempotentByKey).unwrap();
    assert!(
        matches!(
            held,
            PublicationBookkeepingResult::RecoverAllocatedAttempt { .. }
        ),
        "expected the Publication to be held, got {held:?}"
    );

    // Withdrawing proves what the barrier could not: the journal never reached
    // `Dispatching`, so nothing external happened.
    let withdrawn = draft_core::app::publish::withdraw_stalled_attempt(
        &project.workspace,
        &request,
        "the operator cancelled it before it was sent",
    )
    .unwrap();
    assert!(
        matches!(
            withdrawn,
            draft_core::publication::Withdrawn::AllocationWithdrawn { .. }
                | draft_core::publication::Withdrawn::NothingWasSpent { .. }
        ),
        "expected a withdrawal, got {withdrawn:?}"
    );

    // And the Publication is free again.
    assert_eq!(
        bookkeeping(&stores, &publication, DeliverySemantics::IdempotentByKey).unwrap(),
        PublicationBookkeepingResult::Clean,
        "the withdrawal released the Publication"
    );
}

#[test]
fn a_dispatched_attempt_is_never_withdrawn() {
    // The half that must refuse. An attempt past the dispatch boundary may
    // have caused an effect, and withdrawing it would assert that it did not.
    let project = PublishingProject::new();
    let request = project.request("dispatched", DeliverySemantics::IdempotentByKey);

    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        publish(&project.workspace, &request, || {
            panic!("the process died after the boundary was durable")
        })
    }));
    assert!(crashed.is_err());

    let refused = draft_core::app::publish::withdraw_stalled_attempt(
        &project.workspace,
        &request,
        "trying to withdraw something that may already have happened",
    )
    .unwrap();
    assert!(
        matches!(
            refused,
            draft_core::publication::Withdrawn::NotWithdrawable { .. }
        ),
        "a dispatched attempt must never be withdrawn, got {refused:?}"
    );
}

#[test]
fn what_was_later_established_sits_beside_the_outcome_rather_than_replacing_it() {
    // The primary outcome is what Draft could establish at the time. When the
    // truth arrives later it becomes authoritative alongside it, never over
    // it: "we did not know, then we learned" is a different history from "we
    // knew all along", and only one of them explains a retry authorization.
    let project = PublishingProject::new();
    let request = project.request("uncertain-then-known", DeliverySemantics::QueryByClientKey);

    publish(&project.workspace, &request, || {
        DeliveryResult::Undetermined {
            reason: "the connection dropped before the reply".into(),
        }
    })
    .unwrap();

    let digest = draft_core::app::publish::resolve_outcome(
        &project.workspace,
        &request,
        None,
        draft_dcg_contract::publication::PublicationResolutionKind::ResolvedSucceeded {
            external_reference: "remote-confirmed-by-operator".into(),
        },
        "the target's operator confirmed it landed",
    )
    .unwrap();

    // The outcome is untouched.
    let stores = DispatchStores::for_layout(&project.workspace.layout);
    let publication = project.publication(&request);
    let mut found = None;
    for attempt in stores.journals.list().unwrap() {
        let record = stores.journals.read_unlocked(&attempt).unwrap().unwrap();
        if record.publication != publication {
            continue;
        }
        if let Some(reference) = record.state.concluded_attempt() {
            found = stores.outcomes.primary_outcome(reference).unwrap();
        }
    }
    let outcome = found.expect("the attempt concluded");
    assert!(
        matches!(
            outcome.outcome,
            PublicationOutcomeKind::Indeterminate { .. }
        ),
        "the primary outcome must still say what Draft knew at the time: {outcome:?}"
    );

    // And the resolution sits beside it, citing who established it and how.
    let store = draft_core::publication::ResolutionStore::for_layout(&project.workspace.layout);
    let head = store
        .head(&outcome.digest().unwrap())
        .unwrap()
        .expect("the resolution is the head");
    assert_eq!(head.digest().unwrap(), digest);
    assert_eq!(head.rationale, "the target's operator confirmed it landed");
    assert!(head.authority_decision.is_permitted());
    assert!(
        head.supersedes.is_none(),
        "the first resolution for an outcome supersedes nothing"
    );

    // A second one supersedes the first, within the same outcome's chain.
    let second = draft_core::app::publish::resolve_outcome(
        &project.workspace,
        &request,
        None,
        draft_dcg_contract::publication::PublicationResolutionKind::ResolvedFailed {
            reason: "the operator was wrong; the target has no record of it".into(),
        },
        "corrected after checking the target's own log",
    )
    .unwrap();

    let head = store
        .head(&outcome.digest().unwrap())
        .unwrap()
        .expect("the head advanced");
    assert_eq!(head.digest().unwrap(), second);
    assert_eq!(
        head.supersedes,
        Some(digest),
        "a later interpretation names the one it replaced, so the chain stays readable"
    );
}
