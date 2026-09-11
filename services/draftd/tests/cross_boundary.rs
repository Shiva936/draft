//! The whole extension boundary, end to end, through the authoritative daemon.
//!
//! Every other suite proves one layer. This one proves they compose:
//!
//! ```text
//! extension package -> catalog -> install -> enable -> authorize
//!   -> contribution resolution -> draftd read model
//!   -> Console action/read surface -> invoke -> authoritative Draft state
//! ```
//!
//! It lives here, at the daemon, because that is the highest layer involved.
//! Building the signed catalog is shared test-only scaffolding, included by
//! path from the extension-service suite rather than published from a
//! production crate — a lower-level service must not depend upward on `draftd`
//! just so a test can reuse a fixture.
//!
//! The catalog is built by the real packager from the real `/extensions/`
//! sources and signed with an ephemeral key. Nothing here stands in for the
//! shipping artifacts.

use draft_core::app::App;
use draft_core::extension::{ExtensionPermission, NamespacedId, ResourceView};
use draft_core::support::common::OperationId;
use draft_extension_service::{authorization, catalog, contributions, extension};
use draft_ipc::console_application::{
    CanonicalRevisions, ConsoleActionInvocation, ConsoleHandshakeRequest, ConsoleModelRequest,
    ConsoleProtocolVersion, ConsoleSubject, CONSOLE_CAPABILITIES,
};
use draft_ipc::Request;
use draft_sessions::SessionManager;
use draft_store::ServiceStore;
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[path = "../../extension-service/tests/catalog_fixture.rs"]
mod catalog_fixture;
use catalog_fixture::{build_official_catalog, env_lock, trust_catalog, GlobalHome};

/// A workspace with one Rust artifact and a Change containing a change to it.
/// A workspace with one Rust artifact and a sealed revision of an edit to it.
fn rust_workspace() -> (tempfile::TempDir, App, String) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let app = App::new();
    app.init(root).unwrap();

    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/auth.rs"), "fn old() {}\n").unwrap();
    app.checkpoint(root, "base").unwrap();
    std::fs::write(root.join("src/auth.rs"), "pub fn validate_token() {}\n").unwrap();

    // A sealed revision of the edit, which is what verification is about.
    //
    // The files arrived after `init`, so the accepted Baseline has never held
    // them and they have no Resource id to look up. Declaring them by path is
    // how a Change names work on something the project did not start with.
    let scope = ["Cargo.toml".to_string(), "src/auth.rs".to_string()];
    let change = app.dcg_open_change(root, "rust change", &scope).unwrap();
    let revision = app.dcg_seal(root, change.id.as_str()).unwrap();
    (directory, app, revision.id.to_string())
}

/// Install, enable, authorize, use — then disable and revoke, and prove the
/// capability comes and goes without ever corrupting the project.
#[test]
fn the_rust_extension_restores_and_relinquishes_its_capability_through_public_contracts() {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = GlobalHome::new();

    let catalog_dir = tempfile::tempdir().unwrap();
    let fingerprint = match build_official_catalog(catalog_dir.path()) {
        Ok(fingerprint) => fingerprint,
        // `/extensions/` is deliberately absent in the platform-only gate;
        // there is then nothing to install and nothing to prove here.
        Err(error) => {
            eprintln!("skipping: the official packages are unavailable ({error})");
            return;
        }
    };
    trust_catalog("draft-official-test", catalog_dir.path(), &fingerprint);

    let (workspace, app, revision_id) = rust_workspace();
    let root = workspace.path();

    // ---- Before: a generic platform ----------------------------------------
    let before = app.dcg_verify(root, &revision_id).unwrap();
    assert_eq!(
        before.outcome,
        draft_core::evidence::EvidenceOutcome::Unavailable,
        "nothing could check anything before a language extension exists — and \
         that is emphatically not a pass"
    );
    assert!(app
        .resource_tree(root)
        .unwrap()
        .iter()
        .all(|entry| entry.classes.is_empty()));

    // ---- Install and enable ------------------------------------------------
    let installed =
        catalog::install_from_source("draft-official-test", "draft.language.rust", None).unwrap();
    assert!(installed.enabled, "installing enables the package");

    // Installing grants nothing: the profile's data is live, its commands are
    // withheld, and Draft says so.
    let unauthorized = contributions::resolve().unwrap();
    assert!(
        unauthorized
            .withheld
            .iter()
            .any(|withheld| withheld.extension_id == "draft.language.rust"),
        "an unauthorized command-bearing contribution is reported as withheld"
    );

    // Classification is declarative and needs no permission at all.
    let classified = App::with_extension_contributions(std::sync::Arc::new(
        draft_extension_service::contributions::InstalledExtensions,
    ));
    let tree = classified.resource_tree(root).unwrap();
    let source = tree
        .iter()
        .find(|entry| entry.locator.body == "src/auth.rs")
        .expect("the resource is still listed");
    assert_eq!(
        source.classes,
        vec!["draft.language.rust/source".to_string()],
        "the class comes from the contribution, not from a frontend table"
    );
    assert!(source.class_collisions.is_empty());

    let report = classified.classification_report(root).unwrap();
    assert!(report
        .assigned
        .contains(&"draft.language.rust/source".to_string()));
    assert!(
        report.gaps.is_empty(),
        "something is installed to classify now, so there is no gap"
    );

    // ---- Authorize ---------------------------------------------------------
    authorization::authorize(
        "draft.language.rust",
        &[ExtensionPermission::ProcessExecute],
        &OperationId::new("op_cross_boundary".to_string()),
    )
    .unwrap();

    let authorized = contributions::resolve().unwrap();
    assert!(
        authorized.withheld.is_empty(),
        "authorizing releases what was withheld"
    );
    let attributes = BTreeMap::new();
    let view = ResourceView {
        locator_scheme: "file",
        locator_body: "src/auth.rs",
        media_type: None,
        form: None,
        attributes: &attributes,
        content_size: None,
    };
    let classes = authorized.classes_for(&view).assigned;
    let checks = authorized.checks_for(&view, &classes);
    assert!(
        checks
            .get(&NamespacedId::parse("draft.language.rust/suite").unwrap())
            .is_some_and(|check| check.check.operation.executor.command().is_some()),
        "the command is live once authorized"
    );

    // ---- The capability is really back -------------------------------------
    let after = App::with_extension_contributions(std::sync::Arc::new(
        draft_extension_service::contributions::InstalledExtensions,
    ))
    .dcg_verify(root, &revision_id)
    .unwrap();
    assert_ne!(
        after.outcome,
        draft_core::evidence::EvidenceOutcome::Unavailable,
        "the contributed check ran with the extension installed, so the answer \
         is no longer 'nothing could be asked'"
    );
    assert!(
        !after.inputs.is_empty(),
        "evidence names the observations it read"
    );
    // Evidence records the outcome, not a gap list: with a contributing
    // extension installed there is no longer a resource nothing can check, and
    // that is exactly what makes the answer something other than `Unavailable`
    // above. The distinction survives — `Unavailable` still means "nothing
    // existed to ask", which installing something fixes — while the list of
    // which resources it was does not travel in the fact.
    assert!(
        after.revision.as_str() == revision_id,
        "the evidence is about the revision that was verified"
    );

    // ---- Revoke: the executable half goes, the semantic half stays ---------
    authorization::revoke(
        "draft.language.rust",
        None,
        &OperationId::new("op_cross_boundary_revoke".to_string()),
    )
    .unwrap();
    let revoked = contributions::resolve().unwrap();
    assert!(
        revoked
            .withheld
            .iter()
            .any(|withheld| withheld.extension_id == "draft.language.rust"),
        "revoking puts the command-bearing capability back into withheld"
    );
    assert!(
        revoked
            .classes_for(&view)
            .assigned
            .contains(&NamespacedId::parse("draft.language.rust/source").unwrap()),
        "declarative classification survives revocation; only execution is withheld"
    );
    assert!(
        revoked
            .checks_for(&view, &revoked.classes_for(&view).assigned)
            .is_empty(),
        "and the command-backed check goes with the grant"
    );

    // ---- Disable: the contribution goes entirely, the project is fine ------
    extension::set_enabled("draft.language.rust", false).unwrap();
    let disabled = contributions::resolve().unwrap();
    assert!(
        disabled.is_empty(),
        "a disabled extension contributes nothing"
    );
    assert!(disabled.classes_for(&view).assigned.is_empty());

    // The project is still entirely readable and workable.
    let degraded = App::with_extension_contributions(std::sync::Arc::new(
        draft_extension_service::contributions::InstalledExtensions,
    ));
    let tree = degraded.resource_tree(root).unwrap();
    assert!(tree.iter().any(|entry| entry.locator.body == "src/auth.rs"));
    assert!(tree.iter().all(|entry| entry.classes.is_empty()));
    let reverted = degraded.dcg_verify(root, &revision_id).unwrap();
    assert_eq!(
        reverted.outcome,
        draft_core::evidence::EvidenceOutcome::Unavailable,
        "the gap returns rather than the platform failing, and it is not a pass"
    );

    // Re-enabling brings the semantic half back, still unauthorized.
    extension::set_enabled("draft.language.rust", true).unwrap();
    let reenabled = contributions::resolve().unwrap();
    assert!(reenabled
        .classes_for(&view)
        .assigned
        .contains(&NamespacedId::parse("draft.language.rust/source").unwrap()));
    assert!(!reenabled.withheld.is_empty());
}

/// Authorizing an extension from the Console grants exactly what Draft says is
/// pending — and the client never supplies the permission list.
///
/// This is the mutation boundary in one test: the terminal and the browser
/// acknowledge a set the server showed them, and the server reads the set it
/// actually grants from its own state.
#[test]
fn authorizing_from_the_console_grants_the_pending_set_and_nothing_else() {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = GlobalHome::new();

    let catalog_dir = tempfile::tempdir().unwrap();
    let fingerprint = match build_official_catalog(catalog_dir.path()) {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            eprintln!("skipping: the official packages are unavailable ({error})");
            return;
        }
    };
    trust_catalog("draft-official-test", catalog_dir.path(), &fingerprint);
    catalog::install_from_source("draft-official-test", "draft.language.rust", None).unwrap();

    let state = tempfile::tempdir().unwrap();
    let store = ServiceStore::open(state.path().to_path_buf()).unwrap();
    let sessions = SessionManager::new();
    let call = |id: &str, method: &str, params: Value| {
        draftd::dispatch(&store, &sessions, Request::new(id, method, params))
    };

    let session = call(
        "handshake",
        "console.handshake",
        serde_json::to_value(ConsoleHandshakeRequest {
            protocol: ConsoleProtocolVersion::default(),
            client_name: "test".into(),
            client_version: "test".into(),
            client_instance_id: "authorize".into(),
            requested_capabilities: CONSOLE_CAPABILITIES
                .iter()
                .map(|capability| (*capability).to_string())
                .collect(),
        })
        .unwrap(),
    )
    .result
    .unwrap()["application_session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let snapshot = call(
        "snapshot",
        "console.snapshot",
        serde_json::to_value(ConsoleModelRequest {
            application_session_id: session.clone(),
            subject: ConsoleSubject::global(),
        })
        .unwrap(),
    )
    .result
    .unwrap();

    // The shortfall is reported with the remedy the server chose, pointing at
    // an action it actually issued.
    let gaps = snapshot["content"]["capability_gaps"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let gap = gaps
        .iter()
        .find(|gap| gap["extension_id"] == "draft.language.rust")
        .expect("an unauthorized command-bearing extension is a reported shortfall");
    assert_eq!(gap["remedy"], "authorize");
    assert_eq!(gap["remediation_action_id"], "extension.authorize");
    assert!(gap["gap_id"].as_str().is_some_and(|id| !id.is_empty()));

    let authorize = snapshot["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| {
            action["action_id"] == "extension.authorize"
                && action["target"]["id"] == "draft.language.rust"
        })
        .expect("authorizing is offered while something is pending")
        .clone();
    assert!(authorize["enabled"].as_bool().unwrap());
    // The label names the permission being granted, so the acknowledgement is
    // informed rather than blind.
    let inputs = authorize["inputs"].as_array().unwrap();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0]["id"], "acknowledged");
    assert!(inputs[0]["label"]
        .as_str()
        .unwrap()
        .contains("process.execute"));

    let revisions: CanonicalRevisions =
        serde_json::from_value(snapshot["revisions"].clone()).unwrap();

    // An unacknowledged confirmation is refused by the server.
    let refused = call(
        "refused",
        "console.action.invoke",
        serde_json::to_value(ConsoleActionInvocation {
            application_session_id: session.clone(),
            invocation_capability: authorize["invocation_capability"]
                .as_str()
                .unwrap()
                .to_string(),
            operation_id: "op-refused".into(),
            expected_revisions: revisions.clone(),
            arguments: [("acknowledged".to_string(), json!(false))]
                .into_iter()
                .collect(),
        })
        .unwrap(),
    );
    assert!(!refused.ok);
    assert!(refused
        .error
        .unwrap()
        .message
        .contains("must be acknowledged"));
    assert!(
        authorization::view(extension::show("draft.language.rust").unwrap())
            .unwrap()
            .authorized_permissions
            .is_empty(),
        "a refused acknowledgement grants nothing"
    );

    // Acknowledged, it grants exactly the pending set.
    let reissued = call(
        "snapshot-2",
        "console.snapshot",
        serde_json::to_value(ConsoleModelRequest {
            application_session_id: session.clone(),
            subject: ConsoleSubject::global(),
        })
        .unwrap(),
    )
    .result
    .unwrap();
    let token = reissued["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| {
            action["action_id"] == "extension.authorize"
                && action["target"]["id"] == "draft.language.rust"
        })
        .unwrap()["invocation_capability"]
        .as_str()
        .unwrap()
        .to_string();
    let granted = call(
        "granted",
        "console.action.invoke",
        serde_json::to_value(ConsoleActionInvocation {
            application_session_id: session.clone(),
            invocation_capability: token,
            operation_id: "op-granted".into(),
            // The client sends no permission list; it has none to send.
            expected_revisions: revisions,
            arguments: [("acknowledged".to_string(), json!(true))]
                .into_iter()
                .collect(),
        })
        .unwrap(),
    );
    assert!(granted.ok, "{:?}", granted.error);

    let view = authorization::view(extension::show("draft.language.rust").unwrap()).unwrap();
    assert_eq!(
        view.authorized_permissions,
        vec![ExtensionPermission::ProcessExecute],
        "exactly the pending set was granted"
    );
    assert!(view.pending_authorization.is_none());

    // And the capability is really live: the withheld command is back.
    let active = contributions::resolve().unwrap();
    assert!(active.withheld.is_empty());
    let attributes = BTreeMap::new();
    let resource = ResourceView {
        locator_scheme: "file",
        locator_body: "src/main.rs",
        media_type: None,
        form: None,
        attributes: &attributes,
        content_size: None,
    };
    let classes = active.classes_for(&resource).assigned;
    assert!(
        active
            .checks_for(&resource, &classes)
            .values()
            .any(|check| check.check.operation.executor.command().is_some()),
        "the command-backed check is live once the grant exists"
    );

    // The shortfall, and its remedy, are gone from the next read.
    let after = call(
        "snapshot-3",
        "console.snapshot",
        serde_json::to_value(ConsoleModelRequest {
            application_session_id: session,
            subject: ConsoleSubject::global(),
        })
        .unwrap(),
    )
    .result
    .unwrap();
    assert!(after["content"]["capability_gaps"]
        .as_array()
        .map(|gaps| gaps.is_empty())
        .unwrap_or(true));
}

/// Action availability follows `draftd` through the whole lifecycle.
///
/// Web and TUI both render this same action set, so proving it moves with
/// authoritative state proves both frontends do — neither has a rule of its
/// own left to disagree with.
#[test]
fn the_offered_action_set_tracks_authoritative_lifecycle_state() {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = GlobalHome::new();

    let catalog_dir = tempfile::tempdir().unwrap();
    let fingerprint = match build_official_catalog(catalog_dir.path()) {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            eprintln!("skipping: the official packages are unavailable ({error})");
            return;
        }
    };
    trust_catalog("draft-official-test", catalog_dir.path(), &fingerprint);

    let state = tempfile::tempdir().unwrap();
    let store = ServiceStore::open(state.path().to_path_buf()).unwrap();
    let sessions = SessionManager::new();
    let call = |id: &str, method: &str, params: Value| {
        draftd::dispatch(&store, &sessions, Request::new(id, method, params))
    };
    let session = call(
        "handshake",
        "console.handshake",
        serde_json::to_value(ConsoleHandshakeRequest {
            protocol: ConsoleProtocolVersion::default(),
            client_name: "test".into(),
            client_version: "test".into(),
            client_instance_id: "lifecycle-actions".into(),
            requested_capabilities: CONSOLE_CAPABILITIES
                .iter()
                .map(|capability| (*capability).to_string())
                .collect(),
        })
        .unwrap(),
    )
    .result
    .unwrap()["application_session_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Which actions are *enabled* right now, for the Rust extension.
    let enabled_for = |id: &str| -> Vec<String> {
        let snapshot = draftd::dispatch(
            &store,
            &sessions,
            Request::new(
                id,
                "console.snapshot",
                serde_json::to_value(ConsoleModelRequest {
                    application_session_id: session.clone(),
                    subject: ConsoleSubject::global(),
                })
                .unwrap(),
            ),
        )
        .result
        .unwrap();
        let mut offered: Vec<String> = snapshot["actions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|action| {
                action["enabled"] == true && action["target"]["id"] == "draft.language.rust"
            })
            .map(|action| action["action_id"].as_str().unwrap().to_string())
            .collect();
        offered.sort();
        offered
    };

    catalog::install_from_source("draft-official-test", "draft.language.rust", None).unwrap();

    // Installed, enabled, unauthorized: disable and authorize apply; enable
    // does not, because it already is, and revoke does not, because nothing
    // is granted.
    let offered = enabled_for("after-install");
    assert!(offered.contains(&"extension.authorize".to_string()));
    assert!(offered.contains(&"extension.disable".to_string()));
    assert!(!offered.contains(&"extension.enable".to_string()));
    assert!(!offered.contains(&"extension.revoke".to_string()));

    authorization::authorize(
        "draft.language.rust",
        &[ExtensionPermission::ProcessExecute],
        &OperationId::new("op_lifecycle_grant".to_string()),
    )
    .unwrap();

    // Authorized: the grant can be revoked, and there is nothing left pending.
    let offered = enabled_for("after-authorize");
    assert!(offered.contains(&"extension.revoke".to_string()));
    assert!(!offered.contains(&"extension.authorize".to_string()));

    extension::set_enabled("draft.language.rust", false).unwrap();

    // Disabled: the pair flips, without any frontend choosing between them.
    let offered = enabled_for("after-disable");
    assert!(offered.contains(&"extension.enable".to_string()));
    assert!(!offered.contains(&"extension.disable".to_string()));

    extension::set_enabled("draft.language.rust", true).unwrap();
    authorization::revoke(
        "draft.language.rust",
        None,
        &OperationId::new("op_lifecycle_revoke".to_string()),
    )
    .unwrap();

    // Back to needing authorization, and offering it again.
    let offered = enabled_for("after-revoke");
    assert!(offered.contains(&"extension.authorize".to_string()));
    assert!(!offered.contains(&"extension.revoke".to_string()));

    extension::uninstall("draft.language.rust").unwrap();

    // Uninstalled: no action targets it at all, so no frontend renders one.
    assert!(enabled_for("after-uninstall").is_empty());
}
