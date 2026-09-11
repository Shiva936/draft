//! The Change Graph, driven only through the daemon.
//!
//! `draftd` is the authoritative application-service boundary: the CLI,
//! Console Web and Console TUI all reach Draft through these methods. What
//! this proves is that the boundary itself carries the whole chain — that a
//! client with nothing but a socket can take work from an unopened Change to
//! an accepted Baseline and out to an external system, and that the refusals
//! along the way survive the trip.
//!
//! ```text
//! dcg.change.open → dcg.revision.seal → dcg.evidence.record
//!   → dcg.assessment.record → dcg.gate.evaluate → dcg.decision.record
//!   → dcg.promotion.run → dcg.publication.run
//! ```

use draft_core::app::App;
use draft_ipc::Request;
use draft_sessions::SessionManager;
use draft_store::ServiceStore;
use serde_json::{json, Value};

#[path = "../../extension-service/tests/catalog_fixture.rs"]
mod catalog_fixture;
use catalog_fixture::{env_lock, GlobalHome};

/// A registered project with one file and a check that passes.
struct Daemon {
    _home: GlobalHome,
    _state: tempfile::TempDir,
    _project: tempfile::TempDir,
    store: ServiceStore,
    sessions: SessionManager,
    workspace_id: String,
    root: std::path::PathBuf,
}

impl Daemon {
    fn new() -> Self {
        let home = GlobalHome::new();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().to_path_buf();
        std::fs::write(root.join("a.txt"), "hello").unwrap();

        let app = App::new();
        app.init_with_base(&root, "base change").unwrap();
        std::fs::write(
            root.join(".draft/verify.toml"),
            "schema_version = 1\n\n[[checks]]\nname = \"always\"\nenabled = true\n\n\
             [checks.command]\nprogram = \"true\"\nargs = []\n",
        )
        .unwrap();
        let workspace_id = app.open(&root).unwrap().workspace_id.to_string();

        let state = tempfile::tempdir().unwrap();
        Self {
            store: ServiceStore::open(state.path().to_path_buf()).unwrap(),
            sessions: SessionManager::new(),
            _home: home,
            _state: state,
            _project: project,
            workspace_id,
            root,
        }
    }

    /// One daemon call, with the project already named.
    ///
    /// `operation_id` is the client's identity for the call. Mutations are
    /// recorded under it, so passing the same one twice is what a retry after
    /// a lost reply looks like.
    fn call(&self, operation_id: &str, method: &str, mut params: Value) -> Value {
        params["path"] = json!(self.root.display().to_string());
        params["workspace_id"] = json!(self.workspace_id);
        let response = draftd::dispatch(
            &self.store,
            &self.sessions,
            Request::new(operation_id, method, params).with_operation_id(operation_id.to_string()),
        );
        assert!(
            response.ok,
            "{method} failed: {:?}",
            response.error.as_ref().map(|error| error.message.clone())
        );
        response.result.unwrap_or(Value::Null)
    }

    fn refuse(&self, operation_id: &str, method: &str, mut params: Value) -> (String, String) {
        params["path"] = json!(self.root.display().to_string());
        params["workspace_id"] = json!(self.workspace_id);
        let response = draftd::dispatch(
            &self.store,
            &self.sessions,
            Request::new(operation_id, method, params).with_operation_id(operation_id.to_string()),
        );
        let error = response
            .error
            .unwrap_or_else(|| panic!("{method} unexpectedly succeeded: {:?}", response.result));
        (error.code, error.message)
    }

    fn baseline(&self) -> String {
        self.call("read-baseline", "dcg.baseline", json!({}))["baseline"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

#[test]
fn the_daemon_carries_the_whole_chain_to_a_baseline_and_out() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let daemon = Daemon::new();

    let initial = daemon.baseline();
    let scope: Vec<String> = daemon.call("read-graph", "dcg.baseline", json!({}))["composition"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();

    // Publishing before anything is promoted: the initial Baseline exists, but
    // nothing decided work into it, so nothing authorized delivering it.
    let (code, _) = daemon.refuse("premature-publish", "dcg.publication.run", json!({}));
    assert_eq!(code, "REVIEW_REQUIRED");

    let change = daemon.call(
        "open",
        "dcg.change.open",
        json!({ "intent": "edit the file", "scope": scope }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    std::fs::write(daemon.root.join("a.txt"), "changed").unwrap();

    let revision = daemon.call("seal", "dcg.revision.seal", json!({ "change": change }))["id"]
        .as_str()
        .unwrap()
        .to_string();

    let evidence = daemon.call(
        "verify",
        "dcg.evidence.record",
        json!({ "revision": revision }),
    );
    assert_eq!(evidence["outcome"], "passed");

    daemon.call(
        "assess",
        "dcg.assessment.record",
        json!({ "revision": revision, "risk": "low" }),
    );
    let gate = daemon.call("gate", "dcg.gate.evaluate", json!({ "revision": revision }));
    let gate_id = gate["id"].as_str().unwrap().to_string();

    // A satisfied gate is not authority: the Baseline has not moved.
    assert_eq!(daemon.baseline(), initial);

    let decision = daemon.call(
        "decide",
        "dcg.decision.record",
        json!({ "revision": revision, "gate": gate_id, "approve": true }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Nor has an approving decision moved it. Deciding authorizes; promotion
    // accepts.
    assert_eq!(daemon.baseline(), initial);

    // A promotion that does not state the Baseline it believes is accepted is
    // refused at the transport boundary, before it reaches the domain.
    let (code, _) = daemon.refuse(
        "promote-without-precondition",
        "dcg.promotion.run",
        json!({
            "change": change, "revision": revision,
            "decision": decision, "gate": gate_id,
        }),
    );
    assert_eq!(code, "IPC_ERROR");

    let promotion = daemon.call(
        "promote",
        "dcg.promotion.run",
        json!({
            "change": change, "revision": revision,
            "decision": decision, "gate": gate_id,
            "expected_baseline": initial,
        }),
    );
    assert_eq!(promotion["result"], "promoted");
    let promoted = promotion["baseline"].as_str().unwrap().to_string();
    assert_ne!(promoted, initial, "promotion advances the Baseline");
    assert_eq!(daemon.baseline(), promoted);

    let status = daemon.call(
        "promotion-status",
        "dcg.promotion.status",
        json!({ "promotion": promotion["promotion"] }),
    );
    assert_eq!(status["state"], "completed");
    assert_eq!(status["baseline"], Value::String(promoted.clone()));

    // Publishing is its own capability, and the daemon refuses until somebody
    // grants it. Being permitted to accept work into a Baseline is not being
    // permitted to announce it.
    let (code, _) = daemon.refuse("ungranted-publish", "dcg.publication.run", json!({}));
    assert_eq!(code, "CAPABILITY_NOT_AUTHORIZED");
    assert_eq!(
        daemon.baseline(),
        promoted,
        "a refused publication leaves the accepted Baseline exactly as it was"
    );

    let grant = daemon.call("grant", "dcg.publication.grant", json!({}));
    assert_eq!(grant["capability"], "draft.publish/v1");

    // Publication delivers what promotion accepted, and changes nothing.
    let published = daemon.call("publish", "dcg.publication.run", json!({}));
    assert_eq!(published["result"], "concluded");
    assert_eq!(published["outcome"]["outcome"], "succeeded");
    assert_eq!(
        daemon.baseline(),
        promoted,
        "publication has no authority over what the project accepts"
    );

    let publications = daemon.call("publications", "dcg.publication.list", json!({}));
    assert_eq!(publications.as_array().unwrap().len(), 1);
    assert_eq!(publications[0]["state"], "completed");
    assert_eq!(publications[0]["baseline"], Value::String(promoted));
}

#[test]
fn a_client_retry_neither_promotes_nor_delivers_twice() {
    // What a lost reply looks like from a client: the same call, under the
    // same operation id. It must converge on what happened, not repeat it.
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let daemon = Daemon::new();

    let initial = daemon.baseline();
    let scope: Vec<String> = daemon.call("read", "dcg.baseline", json!({}))["composition"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();

    let change = daemon.call(
        "open",
        "dcg.change.open",
        json!({ "intent": "work retried", "scope": scope }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    std::fs::write(daemon.root.join("a.txt"), "retried").unwrap();
    let revision = daemon.call("seal", "dcg.revision.seal", json!({ "change": change }))["id"]
        .as_str()
        .unwrap()
        .to_string();
    daemon.call(
        "verify",
        "dcg.evidence.record",
        json!({ "revision": revision }),
    );
    daemon.call(
        "assess",
        "dcg.assessment.record",
        json!({ "revision": revision, "risk": "low" }),
    );
    let gate_id = daemon.call("gate", "dcg.gate.evaluate", json!({ "revision": revision }))["id"]
        .as_str()
        .unwrap()
        .to_string();
    let decision = daemon.call(
        "decide",
        "dcg.decision.record",
        json!({ "revision": revision, "gate": gate_id, "approve": true }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();

    let promote = |operation: &str| {
        daemon.call(
            operation,
            "dcg.promotion.run",
            json!({
                "change": change, "revision": revision,
                "decision": decision, "gate": gate_id,
                "expected_baseline": initial,
            }),
        )
    };
    let first = promote("promote-attempt");
    // The same operation id: the daemon replays its recorded result rather
    // than running a second promotion.
    let replay = promote("promote-attempt");
    assert_eq!(first, replay, "a replayed operation returns what it did");

    // And a *different* operation id for the same work converges in the
    // domain, because the promotion id is derived from the revision and the
    // parent rather than from whoever asked.
    let again = promote("promote-again");
    assert_eq!(again["result"], "already_promoted");
    assert_eq!(again["baseline"], first["baseline"]);

    let lineage = daemon.call("read-after", "dcg.baseline", json!({}))["lineage"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(lineage, 2, "three requests, one new Baseline");

    // The same for delivery, where repeating it would be a second external
    // effect rather than a second local record.
    daemon.call("grant", "dcg.publication.grant", json!({}));
    let published = daemon.call("publish-attempt", "dcg.publication.run", json!({}));
    assert_eq!(published["result"], "concluded");
    let retried = daemon.call("publish-attempt", "dcg.publication.run", json!({}));
    assert_eq!(
        retried, published,
        "the replayed publication returns what it did"
    );

    // A *different* operation id is not a retry — it is somebody deliberately
    // sending again, which is their right. The engine treats it as a second
    // attempt against the same Publication rather than converging, and the
    // delivery semantics are what make that safe: this target is idempotent by
    // key, so the second send cannot duplicate the effect. A non-idempotent
    // target would refuse instead, from `retry_permission` rather than from
    // anything a client said.
    let fresh_call = daemon.call("publish-again", "dcg.publication.run", json!({}));
    assert_eq!(fresh_call["result"], "concluded");
    assert_ne!(
        fresh_call["attempt"], published["attempt"],
        "a deliberate second send is a second attempt"
    );
    assert_eq!(
        daemon
            .call("publications", "dcg.publication.list", json!({}))
            .as_array()
            .unwrap()
            .len(),
        1,
        "one Publication, whatever the client did: the identity is derived from what the          request is about, not from who asked"
    );
}
