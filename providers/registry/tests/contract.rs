//! Provider contract tests (TEST-002, Blueprint §24.2) run against **every**
//! registered provider: id/capabilities present, detect doesn't panic, open
//! returns a result, and unsupported operations return structured errors.

use draft_core::vcs::errors::ProviderErrorKind;
use draft_core::vcs::types::{CheckpointInput, DiffInput, ProviderFinalizationInput};
use draft_core::workspace;

fn each_provider_satisfies_contract() {
    let reg = draft_providers::default_registry();
    let infos = reg.providers();
    assert!(infos.len() >= 5, "expected git + 4 experimental providers");

    for info in infos {
        let provider = reg.get(&info.id).unwrap();
        // id + capabilities are always available.
        assert_eq!(provider.id(), info.id);
        let _caps = provider.capabilities();

        // detect must not panic on an arbitrary temp dir.
        let dir = tempfile::tempdir().unwrap();
        let _ = provider.detect(dir.path());

        // open against a workspace bound to this provider.
        let ws = workspace::initialize(dir.path(), dir.path(), provider.id(), true).unwrap();
        let repo = provider.open(&ws).unwrap();
        assert_eq!(repo.provider_id(), provider.id());

        // current_view must not panic (Ok or structured Err).
        let _ = repo.current_view();

        // Experimental providers (except fs which supports status) must return
        // structured unsupported errors for finalization.
        if provider.is_experimental() {
            let err = repo
                .prepare_finalization(ProviderFinalizationInput {
                    include_paths: vec![],
                    message: "x".into(),
                    trailers: vec![],
                })
                .unwrap_err();
            assert_eq!(err.kind, ProviderErrorKind::UnsupportedOperation);
        }

        // Every provider's status/diff/conflicts/checkpoint either succeed or
        // return a structured error — never panic.
        let _ = repo.status();
        let _ = repo.diff(DiffInput::WorkingTree);
        let _ = repo.conflicts();
        let _ = repo.create_checkpoint(CheckpointInput { description: None });
    }
}

#[test]
fn all_providers_pass_contract() {
    each_provider_satisfies_contract();
}

#[test]
fn fs_provider_status_scans_files() {
    let reg = draft_providers::default_registry();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hi").unwrap();
    let provider = reg
        .get(&draft_core::vcs::types::ProviderId::new("fs"))
        .unwrap();
    let ws = workspace::initialize(dir.path(), dir.path(), provider.id(), true).unwrap();
    let repo = provider.open(&ws).unwrap();
    let status = repo.status().unwrap();
    assert!(status.entries.iter().any(|e| e.path.as_str() == "note.txt"));
    // Finalization is unsupported for the fs provider.
    assert!(repo
        .prepare_finalization(ProviderFinalizationInput {
            include_paths: vec![],
            message: "x".into(),
            trailers: vec![],
        })
        .is_err());
}

#[test]
fn experimental_providers_are_flagged() {
    let reg = draft_providers::default_registry();
    for info in reg.providers() {
        let experimental_expected = info.id.as_str() != "git";
        assert_eq!(
            info.experimental, experimental_expected,
            "provider {} experimental flag mismatch",
            info.id
        );
    }
}
