//! Unit tests for the provider-neutral foundation (vcs + workspace).

use std::sync::Arc;

use crate::vcs::registry::ProviderRegistry;
use crate::vcs::testing::FakeProvider;
use crate::vcs::{DetectionConfidence, ProviderErrorKind, ProviderId};
use crate::workspace::{self, Workspace};

fn registry() -> ProviderRegistry {
    ProviderRegistry::new().with(Arc::new(FakeProvider))
}

#[test]
fn registry_lists_and_gets_providers() {
    let reg = registry();
    let infos = reg.providers();
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0].id, FakeProvider::id());
    assert!(infos[0].experimental);
    assert!(reg.get(&FakeProvider::id()).is_some());
    assert!(reg.get(&ProviderId::new("nope")).is_none());
}

#[test]
fn detection_selects_fake_provider() {
    let dir = tempfile::tempdir().unwrap();
    let sel = registry().detect(dir.path()).unwrap();
    assert_eq!(sel.detection.provider_id, FakeProvider::id());
    assert_eq!(sel.detection.confidence, DetectionConfidence::Low);
}

#[test]
fn empty_registry_reports_not_detected() {
    let dir = tempfile::tempdir().unwrap();
    let err = ProviderRegistry::new().detect(dir.path()).unwrap_err();
    assert_eq!(err.kind, ProviderErrorKind::NotDetected);
}

#[test]
fn ambiguous_detection_is_reported() {
    // Two fake providers at equal confidence => ambiguous.
    let reg = ProviderRegistry::new()
        .with(Arc::new(FakeProvider))
        .with(Arc::new(FakeProvider));
    let dir = tempfile::tempdir().unwrap();
    let err = reg.detect(dir.path()).unwrap_err();
    assert_eq!(err.kind, ProviderErrorKind::Ambiguous);
}

#[test]
fn workspace_init_open_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let ws = workspace::initialize(root, root, FakeProvider::id(), true).unwrap();
    assert!(root.join(".draft/config.toml").exists());
    assert!(root.join(".draft/workspace.json").exists());
    assert!(root.join(".draft/operations").is_dir());

    // Open from a nested subdirectory should walk up to the workspace root.
    let nested = root.join("a/b");
    std::fs::create_dir_all(&nested).unwrap();
    let opened = Workspace::open(&nested).unwrap();
    assert_eq!(opened.id, ws.id);
    assert_eq!(opened.provider_id, FakeProvider::id());
    assert_eq!(opened.root, root);
    assert!(opened.config.provider.experimental_ack);
}

#[test]
fn open_without_workspace_errors() {
    let dir = tempfile::tempdir().unwrap();
    let err = Workspace::open(dir.path()).unwrap_err();
    assert_eq!(err.kind, crate::error::DraftErrorKind::WorkspaceNotFound);
}

#[test]
fn fake_provider_supports_full_contract() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let ws = workspace::initialize(root, root, FakeProvider::id(), true).unwrap();
    let provider = FakeProvider;
    let repo = crate::vcs::traits::VcsProvider::open(&provider, &ws).unwrap();
    assert_eq!(repo.provider_id(), FakeProvider::id());
    repo.current_view().unwrap();
    repo.status().unwrap();
    repo.diff(Default::default()).unwrap();
    repo.conflicts().unwrap();
    let cp = repo
        .create_checkpoint(crate::vcs::types::CheckpointInput { description: None })
        .unwrap();
    repo.restore_checkpoint((&cp).into()).unwrap();
    let plan = repo
        .prepare_finalization(crate::vcs::types::ProviderFinalizationInput {
            include_paths: vec![],
            message: "m".into(),
            trailers: vec![],
        })
        .unwrap();
    let res = repo.finalize(plan).unwrap();
    assert_eq!(res.object.kind, "snapshot");
}
