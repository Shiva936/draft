//! Frozen inventory of the Console HTTP gateway surface.
//!
//! The browser Console reaches Draft only through these routes, and each
//! mutating route resolves to exactly one authoritative `draftd` method. The
//! extension boundary refactor may only *add* routes, actions and mappings.
//! Anything that disappears — a route, an action name, its daemon method, or an
//! error-to-status mapping — is a compatibility regression and fails here.
//!
//! Additions are deliberate edits to the lists below, reviewed as a product
//! change rather than absorbed as drift.

use super::status_for_error;
use axum::http::StatusCode;

const SOURCE: &str = include_str!("lib.rs");

/// Every path registered on the gateway router, in declaration order.
const ROUTES: &[&str] = &[
    "/",
    "/assets/draft-console.png",
    "/assets/*path",
    "/api/v1/bootstrap",
    "/api/v1/session",
    "/api/v1/events",
    "/api/v1/jobs/:job_id",
    "/api/v1/jobs/:job_id/cancel",
    "/api/v1/system/overview",
    "/api/v1/projects",
    "/api/v1/project-actions/:action",
    "/api/v1/inbox",
    "/api/v1/inbox/:notification_id/:action",
    "/api/v1/doctor",
    "/api/v1/settings",
    "/api/v1/settings/user",
    "/api/v1/console/model",
    "/api/v1/console/actions/invoke",
    "/api/v1/extensions",
    "/api/v1/extensions/sources",
    "/api/v1/extensions/discover",
    "/api/v1/extensions/sources/:source_id/:action",
    "/api/v1/extensions/actions/:action",
    "/api/v1/extensions/:extension_id/:action",
    "/api/v1/search",
    "/api/v1/projects/:workspace_id",
    "/api/v1/projects/:workspace_id/settings",
    "/api/v1/projects/:workspace_id/tasks",
    "/api/v1/projects/:workspace_id/events",
    "/api/v1/projects/:workspace_id/resources",
    "/api/v1/projects/:workspace_id/resource",
    "/api/v1/projects/:workspace_id/classification",
    "/api/v1/projects/:workspace_id/presentation",
    "/api/v1/projects/:workspace_id/tools",
    "/api/v1/projects/:workspace_id/observation-coverage",
    "/api/v1/projects/:workspace_id/observation-provenance",
    "/api/v1/projects/:workspace_id/observation-pending",
    "/api/v1/projects/:workspace_id/observation-preview",
    "/api/v1/projects/:workspace_id/observation-transitions",
    "/api/v1/projects/:workspace_id/intents",
    "/api/v1/projects/:workspace_id/task-templates",
    // The Change Graph. Reads are projections; promote and publish are the two
    // acts that change something, and they take the CSRF check and the
    // client's operation id like every other mutation.
    "/api/v1/projects/:workspace_id/graph",
    "/api/v1/projects/:workspace_id/graph/baseline",
    "/api/v1/projects/:workspace_id/graph/authorization/:change_id/:revision_id",
    "/api/v1/projects/:workspace_id/graph/publications",
    // §8.3's Providers and Baselines sections. Reads only; a binding changes
    // through the audited action path.
    "/api/v1/projects/:workspace_id/providers",
    "/api/v1/projects/:workspace_id/providers/:binding_id",
    "/api/v1/projects/:workspace_id/baselines",
    "/api/v1/projects/:workspace_id/baselines/:baseline_id",
    "/api/v1/projects/:workspace_id/graph/promote",
    "/api/v1/projects/:workspace_id/graph/publish",
    "/api/v1/projects/:workspace_id/actions/:action",
];

/// `(dispatch function, action name, authoritative daemon method)`, sorted.
const ACTION_MAPPINGS: &[(&str, &str, &str)] = &[
    ("extension_action", "authorize", "extension.authorize"),
    ("extension_action", "disable", "extension.disable"),
    ("extension_action", "enable", "extension.enable"),
    ("extension_action", "install", "job.submit"),
    ("extension_action", "revoke", "extension.revoke"),
    ("extension_action", "uninstall", "extension.uninstall"),
    ("extension_action", "update", "job.submit"),
    ("extension_bulk_action", "update-all", "job.submit"),
    ("extension_source_action", "add", "extension.source.add"),
    (
        "extension_source_action",
        "delete",
        "extension.source.delete",
    ),
    (
        "extension_source_action",
        "disable",
        "extension.source.disable",
    ),
    (
        "extension_source_action",
        "enable",
        "extension.source.enable",
    ),
    (
        "extension_source_action",
        "refresh",
        "extension.source.refresh",
    ),
    (
        "extension_source_action",
        "remove",
        "extension.source.remove",
    ),
    ("extension_source_action", "trust", "extension.source.trust"),
    ("notification_action", "dismiss", "notification.dismiss"),
    ("notification_action", "read", "notification.read"),
    ("notification_action", "resolve", "notification.resolve"),
    ("project_action", "candidate-add", "candidate.add"),
    ("project_action", "candidate-remove", "candidate.remove"),
    ("project_action", "candidate-update", "candidate.update"),
    ("project_action", "config-set", "config.set"),
    ("project_action", "config-unset", "config.unset"),
    (
        "project_action",
        "graph-assessment-record",
        "dcg.assessment.record",
    ),
    (
        "project_action",
        "graph-change-abandon",
        "dcg.change.abandon",
    ),
    ("project_action", "graph-change-open", "dcg.change.open"),
    ("project_action", "graph-change-reopen", "dcg.change.reopen"),
    (
        "project_action",
        "graph-decision-record",
        "dcg.decision.record",
    ),
    (
        "project_action",
        "graph-evidence-record",
        "dcg.evidence.record",
    ),
    ("project_action", "graph-gate-evaluate", "dcg.gate.evaluate"),
    ("project_action", "graph-gate-waive", "dcg.gate.waive"),
    (
        "project_action",
        "graph-publication-authorize-retry",
        "dcg.publication.authorize_retry",
    ),
    (
        "project_action",
        "graph-publication-grant",
        "dcg.publication.grant",
    ),
    (
        "project_action",
        "graph-publication-withdraw-attempt",
        "dcg.publication.withdraw_attempt",
    ),
    ("project_action", "graph-review-record", "dcg.review.record"),
    ("project_action", "graph-revision-seal", "dcg.revision.seal"),
    ("project_action", "hook-run", "hook.run"),
    ("project_action", "hook-set", "hook.set"),
    ("project_action", "hook-unset", "hook.unset"),
    ("project_action", "ignore-add", "ignore.add"),
    ("project_action", "ignore-remove", "ignore.remove"),
    ("project_action", "observation-adopt", "observation.adopt"),
    (
        "project_action",
        "resource-commit",
        "resource.workspace.commit",
    ),
    (
        "project_action",
        "resource-create",
        "resource.workspace.stage",
    ),
    (
        "project_action",
        "resource-delete",
        "resource.workspace.stage",
    ),
    (
        "project_action",
        "resource-relocate",
        "resource.workspace.stage",
    ),
    ("project_action", "resource-save", "resource.workspace.save"),
    ("project_action", "task-create", "task.create"),
    ("project_action", "task-drop", "task.drop"),
    (
        "project_action",
        "task-next-action-add",
        "task.next_action.add",
    ),
    (
        "project_action",
        "task-next-action-set",
        "task.next_action.set",
    ),
    ("project_action", "task-update", "task.update"),
    ("project_action", "tool-invoke", "tool.invoke"),
    (
        "system_project_action",
        "adopt-copy",
        "workspace.adopt_copy",
    ),
    ("system_project_action", "init", "workspace.init"),
    ("system_project_action", "register", "workspace.register"),
    ("system_project_action", "relocate", "workspace.relocate"),
    (
        "system_project_action",
        "unregister",
        "workspace.unregister",
    ),
];

/// `(daemon error code, HTTP status)` — the compatibility projection browsers
/// and scripted clients depend on.
const ERROR_STATUSES: &[(&str, StatusCode)] = &[
    ("NOT_FOUND", StatusCode::NOT_FOUND),
    ("WORKSPACE_NOT_FOUND", StatusCode::NOT_FOUND),
    ("CONFLICT_DETECTED", StatusCode::CONFLICT),
    ("OPERATION_IN_PROGRESS", StatusCode::CONFLICT),
    ("UNKNOWN_CONSOLE_SESSION", StatusCode::CONFLICT),
    ("INVALID_CONFIG", StatusCode::BAD_REQUEST),
    ("IPC_ERROR", StatusCode::BAD_REQUEST),
    ("VALIDATION_ERROR", StatusCode::BAD_REQUEST),
    ("UNSUPPORTED_SCHEMA", StatusCode::UNPROCESSABLE_ENTITY),
    ("RISK_POLICY_BLOCKED", StatusCode::FORBIDDEN),
    ("REVIEW_REQUIRED", StatusCode::FORBIDDEN),
    ("PROTECTED_RESOURCE_ACCESS", StatusCode::FORBIDDEN),
];

fn body_of(signature: &str) -> &'static str {
    let start = SOURCE
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} must exist in console/src/lib.rs"));
    let rest = &SOURCE[start + signature.len()..];
    match rest
        .find("\nasync fn ")
        .into_iter()
        .chain(rest.find("\nfn "))
        .min()
    {
        Some(end) => &rest[..end],
        None => rest,
    }
}

fn quoted(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut remainder = text;
    while let Some(open) = remainder.find('"') {
        let after = &remainder[open + 1..];
        let Some(close) = after.find('"') else { break };
        found.push(after[..close].to_string());
        remainder = &after[close + 1..];
    }
    found
}

fn declared_routes() -> Vec<String> {
    body_of("fn router(")
        .split(".route(")
        .skip(1)
        .filter_map(|fragment| quoted(fragment).into_iter().next())
        .collect()
}

/// Expand `"a" | "b" => "method"` arms into one entry per action name.
fn declared_mappings(function: &str) -> Vec<(String, String)> {
    let signature = format!("async fn {function}(");
    body_of(&signature)
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with('"') && trimmed.contains("=>")
        })
        .flat_map(|line| {
            let (actions, method) = line.split_once("=>").expect("filtered above");
            let Some(method) = quoted(method).into_iter().next() else {
                return Vec::new();
            };
            quoted(actions)
                .into_iter()
                .map(|action| (action, method.clone()))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn route_table_matches_the_frozen_inventory() {
    let observed = declared_routes();
    let expected: Vec<String> = ROUTES.iter().map(|route| route.to_string()).collect();

    let removed: Vec<&String> = expected.iter().filter(|r| !observed.contains(r)).collect();
    let added: Vec<&String> = observed.iter().filter(|r| !expected.contains(r)).collect();
    assert!(
        removed.is_empty(),
        "Console routes disappeared (regression): {removed:?}"
    );
    assert!(
        added.is_empty(),
        "new Console routes are not in the frozen inventory: {added:?}. \
         Add them to ROUTES deliberately."
    );
    assert_eq!(
        observed, expected,
        "route declaration order changed; axum matches most-specific first, so \
         reordering can change which handler serves a request"
    );
}

#[test]
fn every_action_still_resolves_to_its_authoritative_daemon_method() {
    let mut observed: Vec<(String, String, String)> = Vec::new();
    for function in [
        "extension_action",
        "extension_bulk_action",
        "extension_source_action",
        "notification_action",
        "project_action",
        "system_project_action",
    ] {
        for (action, method) in declared_mappings(function) {
            observed.push((function.to_string(), action, method));
        }
    }
    observed.sort();

    let expected: Vec<(String, String, String)> = ACTION_MAPPINGS
        .iter()
        .map(|(function, action, method)| {
            (function.to_string(), action.to_string(), method.to_string())
        })
        .collect();

    let removed: Vec<&(String, String, String)> =
        expected.iter().filter(|m| !observed.contains(m)).collect();
    let added: Vec<&(String, String, String)> =
        observed.iter().filter(|m| !expected.contains(m)).collect();
    assert!(
        removed.is_empty(),
        "Console actions disappeared or were remapped (regression): {removed:?}"
    );
    assert!(
        added.is_empty(),
        "new Console actions are not in the frozen inventory: {added:?}. \
         Add them to ACTION_MAPPINGS deliberately."
    );
}

/// Every action the Console offers names a method the daemon dispatches.
///
/// The inventory above and the dispatch table in `lib.rs` are both written by
/// hand, so keeping them in step with each other says nothing about whether
/// the daemon still has the method. `waiver.create` survived its own removal
/// exactly that way: mapped, frozen, agreed on by both halves of this file,
/// and dispatched by nothing — so the Console offered a button whose only
/// possible outcome was an unknown-method error.
#[test]
fn every_mapped_method_is_one_the_daemon_dispatches() {
    let daemon = include_str!("../../services/draftd/src/lib.rs");
    let dispatch = {
        let start = daemon
            .find("fn dispatch_inner(")
            .expect("the daemon has a dispatcher");
        let rest = &daemon[start..];
        let end = rest.find("\n}\n").map_or(rest.len(), |offset| offset + 2);
        &rest[..end]
    };
    let missing: Vec<&str> = ACTION_MAPPINGS
        .iter()
        .map(|(_, _, method)| *method)
        .filter(|method| !dispatch.contains(&format!("\"{method}\"")))
        .collect();
    assert!(
        missing.is_empty(),
        "the Console maps actions to daemon methods that are not dispatched: {missing:?}"
    );
}

#[test]
fn error_status_projection_is_stable() {
    for (code, status) in ERROR_STATUSES {
        assert_eq!(
            status_for_error(code),
            *status,
            "the HTTP status for {code} changed"
        );
    }
    assert_eq!(
        status_for_error("SOMETHING_ELSE"),
        StatusCode::INTERNAL_SERVER_ERROR,
        "unmapped daemon errors must stay 500"
    );
}

#[test]
fn frozen_console_inventory_is_internally_consistent() {
    let mut sorted = ACTION_MAPPINGS.to_vec();
    sorted.sort_unstable();
    assert_eq!(
        ACTION_MAPPINGS,
        sorted.as_slice(),
        "keep ACTION_MAPPINGS sorted"
    );

    // The gateway owns transport only: no action may invent a daemon method
    // that the architecture check cannot find in draftd.
    for (_, _, method) in ACTION_MAPPINGS {
        assert!(
            method.contains('.'),
            "{method} does not look like a draftd method name"
        );
    }
}
