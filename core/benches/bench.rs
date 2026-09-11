//! Performance benchmark suite.
//!
//! Benchmarks the hot operations underlying the core commands across a range of
//! project sizes. Run with `cargo bench`. Regression rule: a >15% slowdown
//! warrants investigation; >25% blocks release unless accepted. See
//! `docs/release-compliance.md`.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use draft_core::app::maintenance;
use draft_core::dcg::compose;
use draft_core::dcg::impact::{ImpactIndex, MergedElements, ResourceElement};

fn crate_resource_id(index: usize) -> draft_core::dcg::resource::ResourceId {
    draft_core::dcg::resource::resource_id_for_locator(&format!("file:src/f{index}.txt"))
}
use draft_core::activity::ActivityLog;
use draft_core::dcg::source_view;
use draft_core::evidence::risk;
use draft_core::evidence::verification;
use draft_core::extension::ProducerRef;
use draft_core::project::layout::DraftLayout;
use draft_core::read_model::index::AffectedPathIndex;
use draft_core::support::hashing;
use draft_core::support::pathguard;
use draft_core::trust::signing::{self, Keypair};

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
    // 10k changed resources is the large-change simulation target.
    for &n in &[100usize, 1000, 5000, 10000] {
        let repo = make_repo(n);
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                source_view::workspace_hash(black_box(repo.path()), &Default::default()).unwrap()
            })
        });
    }
    g.finish();

    // Warm cache: identical digest, cached re-reads.
    let mut g = c.benchmark_group("workspace_hash_cached");
    for &n in &[1000usize, 10000] {
        let repo = make_repo(n);
        let cache = repo.path().join(".draft/cache/hashes/workspace-hash.json");
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        source_view::workspace_hash_cached(repo.path(), &Default::default(), &cache).unwrap();
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                source_view::workspace_hash_cached(
                    black_box(repo.path()),
                    &Default::default(),
                    black_box(&cache),
                )
                .unwrap()
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
    let log = ActivityLog::new(dir.path().join("events"), "prj_000000000001");
    let mut next = 0u64;
    c.bench_function("activity_append", |b| {
        b.iter(|| {
            next += 1;
            log.append(
                &format!("evt_{next:024}"),
                &serde_json::json!({
                    "kind": "ChangeCreated",
                    "subject": "chg_x",
                    "actor": "act_000000000001",
                    "metadata": {},
                }),
            )
            .unwrap()
        })
    });
    c.bench_function("activity_verify_chain", |b| {
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
    use draft_core::extension::{ChangeAspectName, NamespacedId, RiskCondition, RiskRule};

    let rules: Vec<RiskRule> = (0..20)
        .map(|i| RiskRule {
            code: NamespacedId::parse(&format!("bench.rules/rule-{i}")).unwrap(),
            weight: 3,
            when: RiskCondition::AspectCount {
                aspect: ChangeAspectName::ContentChanged,
                at_least: i,
            },
            explanation: format!("rule {i} matched"),
            required_action: None,
        })
        .collect();
    let applicable: Vec<risk::ApplicableRule<'_>> = rules
        .iter()
        .map(|rule| risk::ApplicableRule {
            rule,
            producer: None,
        })
        .collect();

    let mut facts = risk::RiskFacts {
        resource_count: 40,
        ..Default::default()
    };
    facts
        .aspect_counts
        .insert(ChangeAspectName::ContentChanged, 40);
    facts.observation_gaps = 1;

    c.bench_function("risk_assess", |b| {
        b.iter(|| {
            risk::assess(
                black_box(&applicable),
                black_box(&facts),
                risk::RiskThresholds::default(),
                Vec::new(),
            )
        })
    });
}

/// The five-state aggregation over a realistic mixed result set.
fn bench_verification_aggregate(c: &mut Criterion) {
    use draft_core::evidence::verification::{CheckOutcome, VerificationCheckResult};
    use draft_core::extension::{CheckRequirement, CheckSelection, NamespacedId};

    let results: Vec<VerificationCheckResult> = (0..200)
        .map(|i| VerificationCheckResult {
            check_id: NamespacedId::parse(&format!("bench.checks/check-{i}")).unwrap(),
            display_name: format!("check {i}"),
            requirement: if i % 3 == 0 {
                CheckRequirement::Required
            } else {
                CheckRequirement::Optional
            },
            selection: CheckSelection::PerResource,
            reason: "benchmark".into(),
            outcome: match i % 4 {
                0 => CheckOutcome::Passed,
                1 => CheckOutcome::Unavailable {
                    detail: "no capability".into(),
                },
                2 => CheckOutcome::NotEvaluated {
                    detail: "ambiguous".into(),
                },
                _ => CheckOutcome::Failed {
                    detail: "exit 1".into(),
                },
            },
            producer: benchmark_producer(),
            authorization_decision: None,
            executable_identity: None,
            exit_code: None,
            duration_ms: None,
        })
        .collect();
    c.bench_function("verification_aggregate", |b| {
        b.iter(|| verification::aggregate(black_box(&results)))
    });
}

/// The alignment engine over a realistic token stream.
fn bench_sequence_alignment(c: &mut Criterion) {
    use draft_core::execution::mechanism::engine::alignment::{self, AlignmentConfig, Tokenizer};

    let config = AlignmentConfig {
        tokenizer: Tokenizer::Delimited {
            delimiter_bytes: vec![b'\n'],
            include_delimiter: false,
        },
        coordinate_space: "bench/line".into(),
        byte_budget: alignment::DEFAULT_BYTE_BUDGET,
    };
    let mut g = c.benchmark_group("sequence_alignment");
    g.sample_size(20);
    for &n in &[100usize, 1000] {
        let before: String = (0..n).map(|i| format!("line {i}\n")).collect();
        let after: String = (0..n)
            .map(|i| {
                if i % 10 == 0 {
                    format!("CHANGED {i}\n")
                } else {
                    format!("line {i}\n")
                }
            })
            .collect();
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                alignment::compare(
                    black_box(&config),
                    black_box(Some(before.as_bytes())),
                    black_box(Some(after.as_bytes())),
                )
                .unwrap()
            })
        });
    }
    g.finish();
}

/// The producer standing in for one installed, authorized extension.
fn benchmark_producer() -> ProducerRef {
    ProducerRef {
        extension_id: "bench.extension".into(),
        extension_version: "1.0.0".into(),
        package_digest: "sha256:bench-package".into(),
        attestation_digest: "sha256:bench-attestation".into(),
    }
}

fn bench_impact_index(c: &mut Criterion) {
    use draft_core::extension::NamespacedId;

    let index = ImpactIndex::open_memory().unwrap();
    let merged = MergedElements {
        elements: (0..500)
            .map(|i| ResourceElement {
                element_id: format!("element-{i}"),
                resource_id: crate_resource_id(i % 50),
                kind: Some(NamespacedId::parse("bench.extension/unit").unwrap()),
                name: Some(format!("unit {i}")),
                attributes: Default::default(),
                producer: benchmark_producer(),
            })
            .collect(),
        relations: Vec::new(),
        collisions: Vec::new(),
    };
    c.bench_function("impact_index_revision", |b| {
        b.iter(|| {
            index
                .index_revision(black_box("rev_bench000001"), black_box(&merged))
                .unwrap()
        })
    });
    c.bench_function("impact_elements_touched", |b| {
        b.iter(|| {
            index
                .elements_touched_by(black_box("rev_bench000001"))
                .unwrap()
        })
    });
}

/// `n` sealed revisions, each touching one Resource, a quarter of them
/// overlapping their predecessor so conflict detection has real work to do.
fn bench_baseline() -> draft_dcg_contract::BaselineId {
    draft_dcg_contract::BaselineId::new(draft_dcg_contract::Digest::of_bytes(b"bench-baseline"))
}

fn make_members(n: usize) -> Vec<compose::ComposedRevision> {
    (0..n)
        .map(|i| compose::ComposedRevision {
            change: draft_dcg_contract::ids::ChangeId::parse(format!("chg_{i:012}")).unwrap(),
            revision: draft_dcg_contract::ids::ChangeRevisionId::parse(format!("rev_{i:012}"))
                .unwrap(),
            base_baseline: bench_baseline(),
            touched: [crate_resource_id(if i % 4 == 3 { i - 1 } else { i })]
                .into_iter()
                .collect(),
        })
        .collect()
}

/// 1k-revision composition validation.
fn bench_composition(c: &mut Criterion) {
    let mut g = c.benchmark_group("composition_validate");
    g.sample_size(10);
    for &n in &[100usize, 1000] {
        let members = make_members(n);
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                compose::compose(
                    black_box(&bench_baseline()),
                    black_box(&members),
                    compose::relate_by_state,
                )
                .unwrap()
            })
        });
    }
    g.finish();
}

/// Pairwise conflict classification and indexed resource-overlap filtering:
/// conflict detection scales with the number of affected resources.
fn bench_conflict_detection(c: &mut Criterion) {
    let members = make_members(1000);
    c.bench_function("conflict_classify_1k_revisions", |b| {
        b.iter(|| {
            let mut conflicts = 0usize;
            for pair in members.windows(2) {
                if compose::relate_by_state(black_box(&pair[0]), black_box(&pair[1])).relation
                    == compose::Relationship::Conflicting
                {
                    conflicts += 1;
                }
            }
            conflicts
        })
    });

    let mut index = AffectedPathIndex::default();
    for member in &members {
        index.changes.insert(
            member.change.to_string(),
            member.touched.iter().map(ToString::to_string).collect(),
        );
    }
    let candidate = vec![crate_resource_id(500).to_string()];
    c.bench_function("affected_path_index_lookup_1k", |b| {
        b.iter(|| index.changes_touching(black_box(&candidate)))
    });
}

fn bench_gc(c: &mut Criterion) {
    let mut g = c.benchmark_group("gc_cleanup");
    g.sample_size(10);
    g.bench_function("gc_100_disposed_packs", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let paths = DraftLayout::for_root(dir.path());
                paths.create_all().unwrap();
                draft_core::support::fsutil::write_json(
                    &paths.project_json(),
                    &serde_json::json!({
                        "schema_version": 1,
                        "workspace_id": "prj_bench",
                        "draft_version": draft_core::DRAFT_VERSION,
                        "created_at": chrono::Utc::now(),
                    }),
                )
                .unwrap();
                // GC needs an accepted Baseline: it is a collection root, and
                // collecting against a project with no accepted state would
                // measure something Draft never does.
                let home = draft_core::project::home::DraftGlobalStore::locate().unwrap();
                draft_core::trust::identity::global::ensure_actor(&home).unwrap();
                let workspace = draft_core::project::Workspace {
                    workspace_id: draft_dcg_contract::ids::ProjectId::parse("prj_bench").unwrap(),
                    root: dir.path().to_path_buf(),
                    layout: paths.clone(),
                };
                draft_core::app::baseline::accept_current(
                    &draft_core::app::App::new(),
                    &workspace,
                    draft_core::dcg::baseline::BaselineOrigin::Initial,
                )
                .unwrap();
                for i in 0..50 {
                    std::fs::write(paths.tmp_dir().join(format!("orphan{i}")), "x").unwrap();
                }
                (dir, paths)
            },
            |(dir, paths)| {
                let report = maintenance::run(black_box(&paths)).unwrap();
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
    bench_verification_aggregate,
    bench_sequence_alignment,
    bench_impact_index,
    bench_composition,
    bench_conflict_detection,
    bench_gc
);
criterion_main!(benches);
