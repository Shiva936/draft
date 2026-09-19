//! The whole controlled lifecycle, through the CLI, with nothing installed.
//!
//! This is the zero-extension proof at the outermost layer: a plain directory,
//! no domain extensions, no assumptions about what the resources *are*. Draft
//! still observes state, seals a revision of it, records evidence, refuses a
//! gate it cannot satisfy, accepts an explicit human waiver, promotes onto a
//! new Baseline and restores a past state — and where it cannot interpret
//! something it says so rather than guessing.

use assert_cmd::Command as Assert;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

fn draft(dir: &std::path::Path) -> Assert {
    let mut c = Assert::cargo_bin("draft").unwrap();
    c.current_dir(dir);
    // Hermetic, per-test global store (see smoke.rs for rationale).
    c.env("DRAFT_GLOBAL_HOME", dir.join(".draft").join("_global"));
    c
}

fn json(out: std::process::Output) -> serde_json::Value {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn plain_directory_end_to_end_with_waived_gate_and_recovery() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    draft(dir).args(["init"]).assert().success();
    draft(dir)
        .args(["config", "set", "user.name", "E2E"])
        .assert()
        .success();
    draft(dir)
        .args(["config", "set", "user.email", "e2e@example.com"])
        .assert()
        .success();

    std::fs::write(dir.join("app.txt"), "hello\n").unwrap();
    let checkpoint = json(
        draft(dir)
            .args(["pack", "checkpoint", "before change", "--json"])
            .output()
            .unwrap(),
    );
    let snapshot_id = checkpoint["snapshot_id"].as_str().unwrap().to_string();

    // Neither file is in the accepted Baseline — `init` observed an empty
    // directory. A ChangePack may still declare them: adding a Resource is an
    // ordinary change, and naming one by path is how you name something that
    // has no id yet.
    std::fs::write(dir.join("app.txt"), "hello world\n").unwrap();
    std::fs::write(dir.join("notes.txt"), "new file\n").unwrap();
    let change = json(
        draft(dir)
            .args([
                "pack",
                "new",
                "edit the app",
                "--scope",
                "app.txt",
                "notes.txt",
                "--json",
            ])
            .output()
            .unwrap(),
    );
    let change_pack_id = change["id"].as_str().unwrap().to_string();

    let revision = json(
        draft(dir)
            .args(["pack", "revision", "seal", &change_pack_id, "--json"])
            .output()
            .unwrap(),
    );
    let revision_id = revision["id"].as_str().unwrap().to_string();

    // Verification completes and records evidence, and reports `unavailable`:
    // there was no capability to ask, which is a different fact from asking and
    // finding nothing in scope. Either way it is emphatically not a pass, but
    // only this one tells the reader that installing something would change the
    // answer.
    let evidence = json(
        draft(dir)
            .args(["pack", "evidence", "run", &revision_id, "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(evidence["outcome"], serde_json::json!("unavailable"));
    assert_eq!(evidence["revision_pack"], serde_json::json!(revision_id));

    // Two separate refusals, because "nothing verified this" and "nobody
    // assessed the risk" are separate facts. A gate that reported one of them
    // would let the other through unnoticed.
    let gate = json(
        draft(dir)
            .args(["pack", "gates", "evaluate", &revision_id, "--json"])
            .output()
            .unwrap(),
    );
    let unsatisfied: Vec<&str> = gate["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|condition| condition["satisfied"] == serde_json::json!(false))
        .map(|condition| condition["id"].as_str().unwrap())
        .collect();
    assert!(
        unsatisfied.contains(&"draft.gate/verified"),
        "nothing could verify this: {gate}"
    );
    assert!(
        unsatisfied.contains(&"draft.gate/assessed"),
        "nobody assessed this: {gate}"
    );

    // A decision citing an unsatisfied gate authorizes nothing, and promotion
    // has nothing to act on.
    draft(dir)
        .args([
            "pack",
            "decide",
            &revision_id,
            "--approve",
            "--gate",
            gate["id"].as_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(contains("not satisfied"));

    // An explicit human waiver, naming exactly the condition it covers. A
    // waiver excuses a condition only where it is cited, so evaluating without
    // naming it leaves the gate exactly as unsatisfied as before — allowing
    // something has to be a deliberate act at the moment of allowing it.
    let waiver = json(
        draft(dir)
            .args([
                "pack",
                "gates",
                "waive",
                &revision_id,
                "draft.gate/verified",
                "--reason",
                "no verification capability is installed for this project",
                "--days",
                "1",
                "--json",
            ])
            .output()
            .unwrap(),
    );
    let waiver_id = waiver["id"].as_str().unwrap().to_string();

    let gate = json(
        draft(dir)
            .args([
                "pack",
                "gates",
                "evaluate",
                &revision_id,
                "--waiver",
                &waiver_id,
                "--json",
            ])
            .output()
            .unwrap(),
    );
    assert!(
        gate["conditions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == serde_json::json!("draft.gate/verified")
                && c["satisfied"] == serde_json::json!(true)
                && c["detail"]
                    .as_str()
                    .is_some_and(|d| d.contains("waived by"))),
        "a waived condition is satisfied *and* says who allowed it: {gate}"
    );
    assert!(
        gate["conditions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == serde_json::json!("draft.gate/assessed")
                && c["satisfied"] == serde_json::json!(false)),
        "the risk requirement is separate and still unmet: {gate}"
    );

    // Risk is `unassessed` until a person says otherwise. Draft has no built-in
    // notion of what makes a change risky in an arbitrary domain, and does not
    // default to `low`.
    draft(dir)
        .args([
            "pack",
            "assess",
            &revision_id,
            "--risk",
            "low",
            "--rationale",
            "reviewed by hand; nothing installed judges this domain",
        ])
        .assert()
        .success();

    let gate = json(
        draft(dir)
            .args([
                "pack",
                "gates",
                "evaluate",
                &revision_id,
                "--waiver",
                &waiver_id,
                "--json",
            ])
            .output()
            .unwrap(),
    );
    let gate_id = gate["id"].as_str().unwrap().to_string();
    assert!(
        gate["conditions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["satisfied"] == serde_json::json!(true)),
        "every condition is now met or waived: {gate}"
    );

    let decision = json(
        draft(dir)
            .args([
                "pack",
                "decide",
                &revision_id,
                "--approve",
                "--gate",
                &gate_id,
                "--json",
            ])
            .output()
            .unwrap(),
    );

    // Deciding authorizes; promotion is what moves the Baseline. Until it runs
    // the project still accepts what it accepted before.
    let before = json(
        draft(dir)
            .args(["baseline", "show", "--json"])
            .output()
            .unwrap(),
    );
    let promotion = json(
        draft(dir)
            .args([
                "promote",
                &change_pack_id,
                &revision_id,
                "--gate",
                &gate_id,
                "--decision",
                decision["id"].as_str().unwrap(),
                "--json",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(promotion["result"], serde_json::json!("promoted"));
    let after = json(
        draft(dir)
            .args(["baseline", "show", "--json"])
            .output()
            .unwrap(),
    );
    assert_ne!(before["baseline"], after["baseline"]);

    // Promotion appends its preallocated events: the commit, the Baseline it
    // accepted, the ChangePack it completed, the receipt it issued, and the
    // finalization. Naming them is how a reader can tell a promotion that
    // committed from one that only got as far as intending to.
    draft(dir)
        .args(["activity", "list"])
        .assert()
        .success()
        .stdout(
            contains("PromotionCommitted")
                .and(contains("BaselinePromoted"))
                .and(contains("ChangePackCompleted"))
                .and(contains("ReceiptIssued"))
                .and(contains("PromotionFinalized")),
        );

    // Recovery restores target presence *and* target absence, then proves it.
    let recovered = json(
        draft(dir)
            .args(["recover", "run", &snapshot_id, "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(
        recovered["status"],
        serde_json::json!("complete"),
        "recovery must verify the restored state, not merely apply changes: {recovered}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("app.txt")).unwrap(),
        "hello\n"
    );
    assert!(
        !dir.join("notes.txt").exists(),
        "the target proved this resource absent, so recovery removes it"
    );
}
