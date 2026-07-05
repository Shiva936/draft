//! Performance benchmark suite (PRD §11, NFRD §3).
//!
//! Benchmarks the hot operations underlying the ten core commands across a range
//! of repository sizes. Run with `cargo bench`. Regression rule (NFRD §3): a
//! >15% slowdown warrants investigation; >25% blocks release unless accepted.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use draft_core::event::{EventKind, EventLog, NewEvent};
use draft_core::hashing;
use draft_core::layout::ProjectPaths;
use draft_core::lsif::LsifIndex;
use draft_core::pack::PackIntent;
use draft_core::pathguard;
use draft_core::riskv2;
use draft_core::signing::{self, Keypair};
use draft_core::verifyv2::{self, SelectionInput};
use std::collections::BTreeSet;

/// Materialize a temp repo of `n` files for scan/hash benchmarks.
fn make_repo(n: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..n {
        let sub = dir.path().join(format!("src/mod{}", i % 16));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join(format!("f{i}.rs")),
            format!("pub fn f{i}() {{ {i} }}\n"),
        )
        .unwrap();
    }
    dir
}

fn bench_workspace_hash(c: &mut Criterion) {
    let mut g = c.benchmark_group("workspace_hash");
    for &n in &[100usize, 1000, 5000] {
        let repo = make_repo(n);
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| hashing::workspace_hash(black_box(repo.path())).unwrap())
        });
    }
    g.finish();
}

fn bench_hashing(c: &mut Criterion) {
    let value = serde_json::json!({"a": 1, "b": [1,2,3], "c": {"d": "e"}});
    c.bench_function("canonical_json", |b| {
        b.iter(|| hashing::canonical_json(black_box(&value)))
    });
    let data = vec![7u8; 4096];
    c.bench_function("sha256_4k", |b| {
        b.iter(|| hashing::sha256_hex(black_box(&data)))
    });
}

fn bench_events(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let log = EventLog::new(ProjectPaths::for_root(dir.path()));
    c.bench_function("event_append", |b| {
        b.iter(|| {
            log.append(NewEvent {
                kind: EventKind::PackCreated,
                subject_id: Some("pck_x".into()),
                actor_id: "act".into(),
                candidate_id: None,
                workspace_id: "ws".into(),
                receipt_id: Some("rcp_bench".into()),
                metadata: serde_json::json!({}),
            })
            .unwrap()
        })
    });
    c.bench_function("event_verify_chain", |b| {
        b.iter(|| log.verify_chain().unwrap())
    });
}

fn bench_signing(c: &mut Criterion) {
    let kp = Keypair::generate();
    let msg = b"canonical-receipt-bytes-of-moderate-length-0123456789";
    let sig = kp.sign_b64(msg);
    let pk = kp.public_key_b64();
    c.bench_function("ed25519_sign", |b| b.iter(|| kp.sign_b64(black_box(msg))));
    c.bench_function("ed25519_verify", |b| {
        b.iter(|| signing::verify_b64(black_box(&pk), black_box(msg), black_box(&sig)).unwrap())
    });
}

fn bench_pathguard(c: &mut Criterion) {
    c.bench_function("pathguard_check", |b| {
        b.iter(|| {
            let _ = pathguard::check_relative(black_box("src/a/b/c/deep/file.rs"));
            let _ = pathguard::check_relative(black_box("../escape"));
        })
    });
}

fn bench_risk(c: &mut Criterion) {
    let inputs = riskv2::RiskInputs {
        intent: PackIntent::Security,
        files_touched: 40,
        lines_changed: 1500,
        high_risk_paths: vec!["auth".into()],
        public_api_changes: 3,
        semantic_impact: 8,
        ..Default::default()
    };
    c.bench_function("risk_assess", |b| {
        b.iter(|| riskv2::assess(black_box(&inputs)))
    });
}

fn bench_verify_plan(c: &mut Criterion) {
    let input = SelectionInput {
        changed_files: (0..20).map(|i| format!("src/f{i}.rs")).collect(),
        changed_symbols: (0..20).map(|i| format!("sym{i}")).collect(),
        test_files: (0..10).map(|i| format!("tests/t{i}.rs")).collect(),
        fuzz_targets: vec!["parser".into()],
        full: true,
        fuzz: true,
    };
    c.bench_function("verify_plan", |b| {
        b.iter(|| verifyv2::plan(black_box(&input)))
    });
}

fn bench_lsif(c: &mut Criterion) {
    let idx = LsifIndex::open_memory().unwrap();
    let files: Vec<(String, String)> = (0..50)
        .map(|i| {
            (
                format!("src/f{i}.rs"),
                format!("pub fn f{i}() {{}}\nstruct S{i};\n"),
            )
        })
        .collect();
    c.bench_function("lsif_index_pack", |b| {
        b.iter(|| {
            idx.index_pack(black_box("pck_bench"), black_box(&files))
                .unwrap()
        })
    });
    let mut known = BTreeSet::new();
    known.insert("f1".to_string());
    c.bench_function("lsif_record_refs", |b| {
        b.iter(|| {
            idx.record_refs("tests/t.rs", "fn t() { f1(); }", black_box(&known))
                .unwrap()
        })
    });
}

criterion_group!(
    benches,
    bench_workspace_hash,
    bench_hashing,
    bench_events,
    bench_signing,
    bench_pathguard,
    bench_risk,
    bench_verify_plan,
    bench_lsif
);
criterion_main!(benches);
