//! Performance benchmark suite (PRD §11, NFRD §3).
//!
//! Benchmarks the hot operations underlying the ten core commands across a range
//! of repository sizes. Run with `cargo bench`. Regression rule (NFRD §3): a
//! >15% slowdown warrants investigation; >25% blocks release unless accepted.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use draft_core::composition;
use draft_core::event::{EventKind, EventLog, NewEvent};
use draft_core::gc;
use draft_core::hashing;
use draft_core::index::AffectedPathIndex;
use draft_core::layout::ProjectPaths;
use draft_core::lsif::LsifIndex;
use draft_core::pack::{
    ApprovalState, ImportState, PackIntent, PackLockfile, PackManifest, SaveState,
};
use draft_core::pathguard;
use draft_core::risk;
use draft_core::signing::{self, Keypair};
use draft_core::verification::{self, SelectionInput};
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
    // 10k changed files is the NFR-SC-002 large-change simulation target.
    for &n in &[100usize, 1000, 5000, 10000] {
        let repo = make_repo(n);
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| hashing::workspace_hash(black_box(repo.path())).unwrap())
        });
    }
    g.finish();

    // Warm changed-file cache (NFR-PF-001): identical digest, cached re-reads.
    let mut g = c.benchmark_group("workspace_hash_cached");
    for &n in &[1000usize, 10000] {
        let repo = make_repo(n);
        let cache = repo.path().join(".draft/cache/hashes/workspace-hash.json");
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        hashing::workspace_hash_cached(repo.path(), &cache).unwrap();
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                hashing::workspace_hash_cached(black_box(repo.path()), black_box(&cache)).unwrap()
            })
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
    let inputs = risk::RiskInputs {
        intent: PackIntent::Security,
        files_touched: 40,
        lines_changed: 1500,
        high_risk_paths: vec!["auth".into()],
        public_api_changes: 3,
        semantic_impact: 8,
        ..Default::default()
    };
    c.bench_function("risk_assess", |b| {
        b.iter(|| risk::assess(black_box(&inputs)))
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
        b.iter(|| verification::plan(black_box(&input)))
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

/// Build `n` canonical pack lockfiles + manifests. Every fourth pack depends
/// on its predecessor so the graph mixes independent and dependent packs.
fn make_packs(n: usize) -> (Vec<PackManifest>, Vec<PackLockfile>) {
    let mut manifests = Vec::with_capacity(n);
    let mut locks = Vec::with_capacity(n);
    for i in 0..n {
        let id = format!("pck_{i:05}");
        manifests.push(PackManifest {
            schema_version: draft_core::DRAFT_SCHEMA_VERSION.to_string(),
            pack_id: id.clone(),
            name: id.clone(),
            description: String::new(),
            intent: PackIntent::Feature,
            origin: "local".to_string(),
            actor: "bench".to_string(),
            candidate: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            base_workspace_hash: String::new(),
            target_workspace_hash: String::new(),
            changes_hash: String::new(),
            risk_hash: String::new(),
            verify_hash: String::new(),
            lsif_hash: String::new(),
            receipt_hashes: Vec::new(),
            import_state: ImportState::None,
            approval_state: ApprovalState::Pending,
            save_state: SaveState::Unsaved,
        });
        let deps = if i % 4 == 3 {
            vec![format!("pck_{:05}", i - 1)]
        } else {
            Vec::new()
        };
        locks.push(PackLockfile {
            schema_version: draft_core::DRAFT_SCHEMA_VERSION.to_string(),
            pack_id: id.clone(),
            workspace_hash: hashing::sha256_hex(id.as_bytes()),
            file_hashes: [(
                format!("src/mod{i}/file.rs"),
                hashing::sha256_hex(id.as_bytes()),
            )]
            .into_iter()
            .collect(),
            policy_version: draft_core::DRAFT_SCHEMA_VERSION.to_string(),
            risk_engine_version: draft_core::DRAFT_SCHEMA_VERSION.to_string(),
            verification_commands: Vec::new(),
            lsif_version: draft_core::DRAFT_SCHEMA_VERSION.to_string(),
            test_selector_version: draft_core::DRAFT_SCHEMA_VERSION.to_string(),
            fuzz_selector_version: draft_core::DRAFT_SCHEMA_VERSION.to_string(),
            dependency_pack_hashes: deps,
            receipt_hashes: Vec::new(),
        });
    }
    (manifests, locks)
}

/// 1k-active-pack composition validation (NFR-SC-001, SRS-FR-145).
fn bench_composition(c: &mut Criterion) {
    let mut g = c.benchmark_group("composition_validate");
    g.sample_size(10);
    for &n in &[100usize, 1000] {
        let (manifests, locks) = make_packs(n);
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                composition::validate(
                    black_box("rcp_base"),
                    black_box(&manifests),
                    black_box(&locks),
                    Some(Vec::new()),
                )
                .unwrap()
            })
        });
    }
    g.finish();
}

/// Pairwise conflict classification and indexed path-overlap filtering
/// (SRS-FR-146: conflict detection scales with affected paths).
fn bench_conflict_detection(c: &mut Criterion) {
    let (_, locks) = make_packs(1000);
    c.bench_function("conflict_classify_1k_packs", |b| {
        b.iter(|| {
            let mut conflicts = 0usize;
            for pair in locks.windows(2) {
                if composition::classify(black_box(&pair[0]), black_box(&pair[1]))
                    == composition::Relationship::Conflicting
                {
                    conflicts += 1;
                }
            }
            conflicts
        })
    });

    let mut index = AffectedPathIndex::default();
    for lock in &locks {
        index.packs.insert(
            lock.pack_id.clone(),
            lock.file_hashes.keys().cloned().collect(),
        );
    }
    let candidate = vec!["src/mod500/file.rs".to_string()];
    c.bench_function("affected_path_index_lookup_1k", |b| {
        b.iter(|| index.packs_touching(black_box(&candidate)))
    });
}

/// `draft gc` cleanup throughput over disposed pack metadata (NFR-PF-006).
fn bench_gc(c: &mut Criterion) {
    let mut g = c.benchmark_group("gc_cleanup");
    g.sample_size(10);
    g.bench_function("gc_100_disposed_packs", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let paths = ProjectPaths::for_root(dir.path());
                paths.create_all().unwrap();
                let (mut manifests, _) = make_packs(100);
                for m in &mut manifests {
                    m.save_state = SaveState::Saved;
                    std::fs::create_dir_all(paths.pack_dir(&m.pack_id)).unwrap();
                    draft_core::fsutil::write_json(&paths.pack_manifest(&m.pack_id), m).unwrap();
                }
                for i in 0..50 {
                    std::fs::write(paths.tmp_dir().join(format!("orphan{i}")), "x").unwrap();
                }
                (dir, paths)
            },
            |(dir, paths)| {
                let report = gc::run(black_box(&paths)).unwrap();
                drop(dir);
                report
            },
            criterion::BatchSize::PerIteration,
        )
    });
    g.finish();
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
    bench_lsif,
    bench_composition,
    bench_conflict_detection,
    bench_gc
);
criterion_main!(benches);
