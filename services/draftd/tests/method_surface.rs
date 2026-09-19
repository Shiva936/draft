//! Frozen inventory of the authoritative `draftd` IPC surface.
//!
//! `draftd` is the only authoritative boundary: CLI, Console Web and Console
//! TUI all reach Draft through these methods. The extension boundary refactor
//! may only *add* methods, so both the dispatch set and the read-only
//! (non-mutation) classification are frozen here. A method that disappears, or
//! silently changes its mutation classification, fails before release.
//!
//! Additions are intentional edits to the lists below, reviewed as a product
//! change rather than absorbed as drift.

use draft_ipc::Request;
use draft_sessions::SessionManager;
use draft_store::ServiceStore;

/// Handled ahead of `dispatch_inner`, so they never appear as match arms.
const PRE_DISPATCH_METHODS: &[&str] = &["console.handshake", "service.handshake"];

/// Every method reachable through `dispatch_inner`, sorted.
const DISPATCH_METHODS: &[&str] = &[
    "candidate.add",
    "candidate.list",
    "candidate.remove",
    "candidate.update",
    "checkpoint.create",
    "classification.bundle",
    "config.global.update",
    "config.list",
    "config.set",
    "config.unset",
    "console.action.invoke",
    "console.doctor",
    "console.inbox",
    "console.overview",
    "console.project",
    "console.project.settings",
    "console.search",
    "console.settings",
    "console.snapshot",
    "console.watch",
    "dcg.assessment.record",
    "dcg.authorization",
    "dcg.baseline",
    "dcg.baseline.list",
    "dcg.baseline.show",
    "dcg.change_pack.abandon",
    "dcg.change_pack.conflicts",
    "dcg.change_pack.coverage",
    "dcg.change_pack.impact",
    "dcg.change_pack.intent",
    "dcg.change_pack.list",
    "dcg.change_pack.open",
    "dcg.change_pack.receipts",
    "dcg.change_pack.recovery",
    "dcg.change_pack.reopen",
    "dcg.change_pack.representation",
    "dcg.change_pack.scope",
    "dcg.decision.record",
    "dcg.evidence.record",
    "dcg.gate.evaluate",
    "dcg.gate.waive",
    "dcg.project",
    "dcg.promotion.run",
    "dcg.promotion.status",
    "dcg.publication.authorize_retry",
    "dcg.publication.grant",
    "dcg.publication.list",
    "dcg.publication.run",
    "dcg.publication.withdraw_attempt",
    "dcg.review.record",
    "dcg.revision_pack.seal",
    "doctor.project",
    "events.canonical",
    "events.list",
    "events.replay",
    "events.verify",
    "execution.list",
    "execution.show",
    "extension.authorize",
    "extension.disable",
    "extension.enable",
    "extension.install",
    "extension.list",
    "extension.revoke",
    "extension.search",
    "extension.show",
    "extension.source.add",
    "extension.source.delete",
    "extension.source.disable",
    "extension.source.enable",
    "extension.source.list",
    "extension.source.refresh",
    "extension.source.remove",
    "extension.source.show",
    "extension.source.trust",
    "extension.uninstall",
    "extension.update",
    "extension.update_all",
    "hook.list",
    "hook.run",
    "hook.set",
    "hook.unset",
    "ignore.add",
    "ignore.list",
    "ignore.remove",
    "inbox.list",
    "index.rebuild",
    "intent.list",
    "job.cancel",
    "job.list",
    "job.status",
    "job.submit",
    "notification.dismiss",
    "notification.list",
    "notification.read",
    "notification.resolve",
    "observation.adopt",
    "observation.context",
    "observation.coverage",
    "observation.pending",
    "observation.preview",
    "observation.provenance",
    "observation.transitions",
    "operation.cancel",
    "operation.status",
    "presentation.bindings",
    "project.provider.list",
    "project.provider.show",
    "receipt.list",
    "receipt.show",
    "resource.create",
    "resource.delete",
    "resource.get",
    "resource.list",
    "resource.relocate",
    "resource.search",
    "resource.workspace",
    "resource.workspace.commit",
    "resource.workspace.save",
    "resource.workspace.show",
    "resource.workspace.stage",
    "rollback.run",
    "service.ping",
    "service.shutdown",
    "service.status",
    "service.telemetry",
    "task.create",
    "task.drop",
    "task.list",
    "task.next_action.add",
    "task.next_action.set",
    "task.show",
    "task.templates",
    "task.update",
    "task.views",
    "tool.invoke",
    "tool.list",
    "workspace.adopt_copy",
    "workspace.init",
    "workspace.list",
    "workspace.register",
    "workspace.relocate",
    "workspace.status",
    "workspace.unregister",
];

/// Methods classified read-only, so they bypass operation idempotency and
/// finalization. Moving a method in or out of this set changes observable
/// daemon behaviour and must be a deliberate edit.
const READ_ONLY_METHODS: &[&str] = &[
    "candidate.list",
    "classification.bundle",
    "config.list",
    "console.doctor",
    "console.inbox",
    "console.overview",
    "console.project",
    "console.project.settings",
    "console.search",
    "console.settings",
    "console.snapshot",
    "console.watch",
    "dcg.authorization",
    "dcg.baseline",
    "dcg.baseline.list",
    "dcg.baseline.show",
    "dcg.change_pack.conflicts",
    "dcg.change_pack.coverage",
    "dcg.change_pack.intent",
    "dcg.change_pack.list",
    "dcg.change_pack.receipts",
    // Reporting where an interrupted promotion stands is a read. Performing
    // the recovery is not this method, and the classifier it uses counts
    // nothing precisely so that reading cannot look like recovering.
    "dcg.change_pack.recovery",
    "dcg.change_pack.representation",
    "dcg.change_pack.scope",
    "dcg.project",
    "dcg.promotion.status",
    "dcg.publication.list",
    "events.list",
    "events.verify",
    "execution.list",
    "execution.show",
    "extension.list",
    "extension.search",
    "extension.show",
    "extension.source.list",
    "extension.source.show",
    "hook.list",
    "ignore.list",
    "job.list",
    "job.status",
    "notification.list",
    "observation.context",
    "observation.coverage",
    "observation.pending",
    "observation.preview",
    "observation.provenance",
    "observation.transitions",
    "operation.status",
    // Provider state is a read. Bindings change only through the audited
    // Console action path, which revalidates freshness before it mutates.
    "project.provider.list",
    "project.provider.show",
    "receipt.list",
    "receipt.show",
    "resource.get",
    "resource.list",
    "resource.search",
    "resource.workspace",
    "resource.workspace.show",
    "service.ping",
    "service.status",
    "service.telemetry",
    "task.list",
    "task.show",
    "workspace.list",
    "workspace.status",
];

fn body_of<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} must exist in services/draftd/src/lib.rs"));
    let rest = &source[start + signature.len()..];
    match rest.find("\nfn ") {
        Some(end) => &rest[..end],
        None => rest,
    }
}

fn quoted(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut remainder = line;
    while let Some(open) = remainder.find('"') {
        let after = &remainder[open + 1..];
        let Some(close) = after.find('"') else { break };
        found.push(after[..close].to_string());
        remainder = &after[close + 1..];
    }
    found
}

/// Match arms are the lines whose first token is a quoted method literal.
fn arm_methods(body: &str) -> Vec<String> {
    let mut methods: Vec<String> = body
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with('"') && trimmed.contains("=>")
        })
        .flat_map(|line| quoted(line.split("=>").next().unwrap_or("")))
        .collect();
    methods.sort();
    methods.dedup();
    methods
}

fn source() -> String {
    std::fs::read_to_string("src/lib.rs").expect("draftd source is readable")
}

#[test]
fn dispatch_surface_matches_the_frozen_inventory() {
    let source = source();
    let observed = arm_methods(body_of(&source, "fn dispatch_inner("));
    let expected: Vec<String> = DISPATCH_METHODS.iter().map(|m| m.to_string()).collect();

    let removed: Vec<&String> = expected.iter().filter(|m| !observed.contains(m)).collect();
    let added: Vec<&String> = observed.iter().filter(|m| !expected.contains(m)).collect();
    assert!(
        removed.is_empty(),
        "daemon methods disappeared (regression): {removed:?}"
    );
    assert!(
        added.is_empty(),
        "new daemon methods are not in the frozen inventory: {added:?}. \
         Add them to DISPATCH_METHODS deliberately."
    );
}

/// The methods that get a finalization record, frozen.
///
/// A client that loses the reply to one of these must be able to ask what
/// happened rather than repeat it. Both directions of drift matter: a method
/// added here without thought pays for bookkeeping it does not need, and one
/// that disappears from the dispatcher but stays listed here is a name that
/// classifies nothing — which is how the list stops describing the daemon.
const IRREVERSIBLE_METHODS: &[&str] = &[
    "dcg.promotion.run",
    "dcg.publication.run",
    "rollback.run",
    "workspace.adopt_copy",
    "workspace.relocate",
];

#[test]
fn irreversible_classification_matches_the_frozen_inventory() {
    let source = source();
    let mut observed = quoted(body_of(&source, "fn has_irreversible_boundary("));
    observed.sort();
    observed.dedup();
    let expected: Vec<String> = IRREVERSIBLE_METHODS.iter().map(|m| m.to_string()).collect();
    assert_eq!(
        observed, expected,
        "the finalization boundary set changed; a method moving in or out of it \
         changes whether a client can ask what happened after losing a reply"
    );

    // And every one of them is a method the daemon actually dispatches. A name
    // here that nothing dispatches classifies nothing at all.
    let dispatched = arm_methods(body_of(&source, "fn dispatch_inner("));
    for method in &observed {
        assert!(
            dispatched.contains(method),
            "'{method}' is classified as irreversible but the daemon does not dispatch it"
        );
    }
}

#[test]
fn read_only_classification_matches_the_frozen_inventory() {
    let source = source();
    let mut observed = quoted(body_of(&source, "fn is_mutation("));
    observed.sort();
    observed.dedup();
    let expected: Vec<String> = READ_ONLY_METHODS.iter().map(|m| m.to_string()).collect();
    assert_eq!(
        observed, expected,
        "the read-only method set changed; a method moving in or out of it \
         changes idempotency and finalization behaviour"
    );

    // Updating both lists together would hide a name the daemon stopped
    // dispatching, which is how `resource.compare` survived its own removal:
    // classified read-only, dispatched by nothing, listed as if it existed.
    let dispatched = arm_methods(body_of(&source, "fn dispatch_inner("));
    for method in &observed {
        assert!(
            dispatched.contains(method),
            "'{method}' is classified read-only but the daemon does not dispatch it"
        );
    }
}

#[test]
fn frozen_methods_are_internally_consistent() {
    let mut sorted = DISPATCH_METHODS.to_vec();
    sorted.sort_unstable();
    assert_eq!(DISPATCH_METHODS, sorted.as_slice(), "keep the list sorted");

    let mut sorted = READ_ONLY_METHODS.to_vec();
    sorted.sort_unstable();
    assert_eq!(READ_ONLY_METHODS, sorted.as_slice(), "keep the list sorted");

    for method in READ_ONLY_METHODS {
        assert!(
            DISPATCH_METHODS.contains(method),
            "read-only method {method} is not dispatchable"
        );
    }
    for method in PRE_DISPATCH_METHODS {
        assert!(
            source().contains(&format!("req.method == \"{method}\"")),
            "{method} must still be handled ahead of dispatch_inner"
        );
    }
}

#[test]
fn every_read_only_method_is_reachable_and_unknown_methods_are_rejected() {
    let state = tempfile::tempdir().unwrap();
    let store = ServiceStore::open(state.path().to_path_buf()).unwrap();
    let sessions = SessionManager::new();

    // Read-only methods are safe to probe: they never mutate durable state, so
    // this is a real reachability check rather than a source scan.
    for method in READ_ONLY_METHODS {
        let response = draftd::dispatch(
            &store,
            &sessions,
            Request::new(format!("surface-{method}"), *method, serde_json::json!({})),
        );
        if let Some(error) = response.error {
            assert_ne!(
                error.code, "UNKNOWN_METHOD",
                "{method} is no longer dispatchable"
            );
        }
    }

    let response = draftd::dispatch(
        &store,
        &sessions,
        Request::new("surface-unknown", "no.such.method", serde_json::json!({})),
    );
    assert_eq!(
        response.error.expect("unknown methods must fail").code,
        "UNKNOWN_METHOD",
        "the reachability probe above is only meaningful if unknown methods are rejected"
    );
}
