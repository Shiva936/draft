//! Installation and authorization are separate decisions.
//!
//! These tests drive the real install path against a temporary global store and
//! assert the four separations hold: installing never authorizes, an
//! unauthorized package is still installed and enabled with its static
//! contributions live, a grant binds to one exact artifact, and any update
//! retires the grant it invalidates even when the new build asks for exactly
//! the same permission.

use draft_core::extension::{
    ExtensionContribution, ExtensionContributionKind, ExtensionId, ExtensionManifest,
    ExtensionPermission,
};
use draft_core::support::common::OperationId;
use draft_core::support::fsutil::{ensure_dir, write_json};
use draft_extension_service::{authorization, extension};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// The global store is selected by process environment, so these tests take
/// turns rather than racing over one another's installations.
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct GlobalHome {
    _directory: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
}

impl GlobalHome {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("DRAFT_GLOBAL_HOME");
        std::env::set_var("DRAFT_GLOBAL_HOME", directory.path().join(".draft"));
        Self {
            _directory: directory,
            previous,
        }
    }
}

impl Drop for GlobalHome {
    fn drop(&mut self) {
        match &self.previous {
            Some(previous) => std::env::set_var("DRAFT_GLOBAL_HOME", previous),
            None => std::env::remove_var("DRAFT_GLOBAL_HOME"),
        }
    }
}

/// A package that declares one declarative contribution and one command-backed
/// check, so a single install exercises both halves of the boundary.
fn write_package(root: &Path, version: &str, permissions: Vec<ExtensionPermission>) {
    ensure_dir(&root.join("contributions")).unwrap();
    ensure_dir(&root.join("docs")).unwrap();
    let manifest = ExtensionManifest {
        schema_version: draft_core::contracts::current_version(
            draft_core::contracts::ContractId::ExtensionManifest,
        ),
        id: ExtensionId::parse("example.language").unwrap(),
        name: "Example language".into(),
        version: version.into(),
        publisher: "example".into(),
        draft_api: format!("^{}", draft_core::DRAFT_API_VERSION),
        contributions: vec![
            ExtensionContribution {
                id: "classification".into(),
                kind: ExtensionContributionKind::ResourceClassification,
                path: "contributions/classification.json".into(),
            },
            ExtensionContribution {
                id: "verification".into(),
                kind: ExtensionContributionKind::Verification,
                path: "contributions/verification.json".into(),
            },
        ],
        permissions,
        description: Some("Example language support".into()),
        keywords: vec!["example".into()],
        documentation: vec!["docs/readme.md".into()],
        licenses: vec!["LICENSE.txt".into()],
        assets: vec![],
        schemas: vec![],
    };
    write_json(&root.join("extension.json"), &manifest).unwrap();
    std::fs::write(
        root.join("contributions/classification.json"),
        serde_json::to_vec(&serde_json::json!({
            "class_id": "example.language/source",
            "display_name": "Example",
            "applies_to": {"predicate": "path_suffix", "suffix": ".ex"},
            "attributes": []
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.join("contributions/verification.json"),
        serde_json::to_vec(&serde_json::json!({
            "checks": [{
                "check_id": "example.language/suite",
                "display_name": "Example suite",
                "applies_to": {
                    "predicate": "has_class",
                    "class_id": "example.language/source"
                },
                "requirement": "required",
                "selection": "whole",
                "operation": {
                    "request_contract": {
                        "schema_id": "draft.core/verification-request",
                        "revision": 1
                    },
                    "response_contract": {
                        "schema_id": "draft.core/verification-result",
                        "revision": 1
                    },
                    "max_response_bytes": 65536,
                    "executor": {
                        "kind": "command",
                        "command": {"program": "example-test", "args": ["--all"]}
                    }
                }
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(root.join("docs/readme.md"), format!("version {version}")).unwrap();
    std::fs::write(root.join("LICENSE.txt"), "license").unwrap();
}

fn operation(name: &str) -> OperationId {
    OperationId::new(format!("op_{name}"))
}

#[test]
fn installing_never_authorizes_and_an_unauthorized_package_still_works() {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = GlobalHome::new();
    let package = tempfile::tempdir().unwrap();
    write_package(
        package.path(),
        "1.0.0",
        vec![ExtensionPermission::ProcessExecute],
    );

    let installed = extension::install(package.path()).unwrap();
    assert!(installed.enabled, "a fresh install is enabled");
    assert_eq!(installed.version(), "1.0.0");

    // Installation created no grant at all.
    assert!(authorization::authorized_permissions(&installed)
        .unwrap()
        .is_empty());
    assert!(!authorization::authorizes(&installed, ExtensionPermission::ProcessExecute).unwrap());

    // The package is installed, enabled and listed — the missing authorization
    // is reported, not treated as an installation failure.
    let listed = extension::list().unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].enabled);
    let pending = authorization::pending(&installed).unwrap().unwrap();
    assert_eq!(
        pending.missing_permissions,
        vec![ExtensionPermission::ProcessExecute]
    );
    assert!(
        !pending.superseded_by_update,
        "a first install is not an update"
    );

    // Its static contributions are active regardless.
    let contributions = extension::active_contributions().unwrap();
    assert!(contributions
        .iter()
        .any(|(_, contribution)| contribution.kind
            == ExtensionContributionKind::ResourceClassification));
}

#[test]
fn a_grant_covers_only_the_declared_permissions_of_the_installed_artifact() {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = GlobalHome::new();
    let package = tempfile::tempdir().unwrap();
    write_package(package.path(), "1.0.0", vec![]);

    let installed = extension::install(package.path()).unwrap();
    // The package declares no permissions, so there is nothing outstanding …
    assert!(authorization::pending(&installed).unwrap().is_none());
    // … and nothing that can be granted either.
    let refused = authorization::authorize(
        "example.language",
        &[ExtensionPermission::ProcessExecute],
        &operation("grant"),
    )
    .unwrap_err();
    assert!(
        refused.to_string().contains("does not declare permission"),
        "unexpected refusal: {refused}"
    );
}

#[test]
fn a_new_artifact_at_the_same_extension_id_starts_unauthorized() {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = GlobalHome::new();
    let package = tempfile::tempdir().unwrap();
    write_package(
        package.path(),
        "1.0.0",
        vec![ExtensionPermission::ProcessExecute],
    );

    let installed = extension::install(package.path()).unwrap();
    authorization::authorize(
        "example.language",
        &[ExtensionPermission::ProcessExecute],
        &operation("grant"),
    )
    .unwrap();
    assert!(authorization::authorizes(&installed, ExtensionPermission::ProcessExecute).unwrap());

    // Installing over an existing install is refused; replacing a directly
    // installed package means removing it first. Both steps retire the grant.
    let updated_package = tempfile::tempdir().unwrap();
    write_package(
        updated_package.path(),
        "2.0.0",
        vec![ExtensionPermission::ProcessExecute],
    );
    assert!(extension::install(updated_package.path()).is_err());

    extension::uninstall("example.language").unwrap();
    let updated = extension::install(updated_package.path()).unwrap();
    assert_eq!(updated.version(), "2.0.0");

    // The new artifact asks for exactly the same permission and still holds
    // nothing: a grant is bound to an artifact, not to an extension id.
    assert!(!authorization::authorizes(&updated, ExtensionPermission::ProcessExecute).unwrap());
    let pending = authorization::pending(&updated).unwrap().unwrap();
    assert_eq!(
        pending.missing_permissions,
        vec![ExtensionPermission::ProcessExecute]
    );
    assert!(
        pending.superseded_by_update,
        "the user authorized this extension before, so say so"
    );

    // Re-authorizing the new artifact restores it.
    authorization::authorize(
        "example.language",
        &[ExtensionPermission::ProcessExecute],
        &operation("regrant"),
    )
    .unwrap();
    assert!(authorization::authorizes(&updated, ExtensionPermission::ProcessExecute).unwrap());
    assert!(authorization::pending(&updated).unwrap().is_none());
}

#[test]
fn revoking_leaves_the_package_installed_and_enabled() {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = GlobalHome::new();
    let package = tempfile::tempdir().unwrap();
    write_package(
        package.path(),
        "1.0.0",
        vec![ExtensionPermission::ProcessExecute],
    );

    let installed = extension::install(package.path()).unwrap();
    authorization::authorize(
        "example.language",
        &[ExtensionPermission::ProcessExecute],
        &operation("grant"),
    )
    .unwrap();

    let withdrawn = authorization::revoke("example.language", None, &operation("revoke")).unwrap();
    assert_eq!(withdrawn.artifact.package_version, "1.0.0");
    assert!(!authorization::authorizes(&installed, ExtensionPermission::ProcessExecute).unwrap());

    // Revoking a capability is not uninstalling: the package and its static
    // contributions are untouched.
    let listed = extension::list().unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].enabled);
    assert!(!extension::active_contributions().unwrap().is_empty());

    // Revoking twice is a clear refusal, not a silent success.
    assert!(authorization::revoke("example.language", None, &operation("revoke-again")).is_err());
}

#[test]
fn uninstalling_retires_authorization_but_keeps_provenance() {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = GlobalHome::new();
    let package = tempfile::tempdir().unwrap();
    write_package(
        package.path(),
        "1.0.0",
        vec![ExtensionPermission::ProcessExecute],
    );

    let installed = extension::install(package.path()).unwrap();
    authorization::authorize(
        "example.language",
        &[ExtensionPermission::ProcessExecute],
        &operation("grant"),
    )
    .unwrap();

    let removed = extension::uninstall("example.language").unwrap();
    assert_eq!(removed.version(), "1.0.0");
    assert!(!authorization::authorizes(&installed, ExtensionPermission::ProcessExecute).unwrap());
    assert!(extension::list().unwrap().is_empty());

    // Reinstalling the identical artifact does not resurrect the old grant.
    let reinstalled = extension::install(package.path()).unwrap();
    assert!(!authorization::authorizes(&reinstalled, ExtensionPermission::ProcessExecute).unwrap());
}

/// A package declaring a contribution this build cannot consume is refused.
///
/// This build consumes every kind the format defines, so the case is exercised
/// with a kind that does not exist here at all — an author targeting a newer
/// Draft. The property under test is the refusal, not the particular kind:
/// installing a package whose declared contribution nothing would act on would
/// make "installed and enabled" a false statement about it.
#[test]
fn a_contribution_this_build_cannot_consume_is_refused_at_install() {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = GlobalHome::new();
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().to_path_buf();
    write_package(&package, "1.0.0", vec![]);

    let manifest_path = package.join("extension.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["contributions"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": "future",
            "kind": "resource_projection",
            "path": "contributions/future.json"
        }));
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    std::fs::write(
        package.join("contributions/future.json"),
        serde_json::to_vec(&serde_json::json!({"title": "example"})).unwrap(),
    )
    .unwrap();

    // Draft refuses it, names the offending contribution, and installs nothing.
    let error = draft_extension_service::extension::install(&package).unwrap_err();
    assert!(
        error.message.contains("resource_projection") || error.message.contains("future"),
        "the refusal must name what it could not consume: {}",
        error.message
    );
    assert!(
        draft_extension_service::extension::list()
            .unwrap()
            .is_empty(),
        "a refused package must leave no installed record"
    );

    // And nothing partially activated: the supported contributions of a
    // refused package contribute nothing either.
    assert!(draft_extension_service::contributions::resolve()
        .unwrap()
        .is_empty());
}

/// Two installed extensions claiming the same artifact.
///
/// Agreement and conflict are different outcomes, and neither is resolved by
/// whichever package happened to be read first. This is the property the
/// future multi-source ecosystem depends on: independent publishers covering
/// the same file suffix is normal, not an edge case.
#[test]
fn competing_installed_extensions_agree_or_conflict_but_never_race() {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    /// A minimal package contributing one file association.
    fn write_association(root: &Path, id: &str, class: &str, display: &str) {
        ensure_dir(&root.join("contributions")).unwrap();
        ensure_dir(&root.join("docs")).unwrap();
        std::fs::write(
            root.join("contributions/classification.json"),
            serde_json::to_vec(&serde_json::json!({
                "class_id": class,
                "display_name": display,
                "applies_to": {"predicate": "path_suffix", "suffix": ".ex"},
                "attributes": []
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(root.join("docs/readme.md"), "docs").unwrap();
        std::fs::write(root.join("LICENSE.txt"), "license").unwrap();
        std::fs::write(
            root.join("extension.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": draft_core::contracts::current_version(
                    draft_core::contracts::ContractId::ExtensionManifest
                ),
                "id": id,
                "name": display,
                "version": "1.0.0",
                "publisher": "example",
                "draft_api": format!("^{}", draft_core::DRAFT_API_VERSION),
                "contributions": [{
                    "id": "classification",
                    "kind": "resource_classification",
                    "path": "contributions/classification.json"
                }],
                "permissions": [],
                "documentation": ["docs/readme.md"],
                "licenses": ["LICENSE.txt"],
                "assets": [],
                "schemas": []
            }))
            .unwrap(),
        )
        .unwrap();
    }

    let install = |id: &str, class: &str, display: &str| {
        let directory = tempfile::tempdir().unwrap();
        write_association(directory.path(), id, class, display);
        draft_extension_service::extension::install(directory.path()).unwrap();
        directory
    };

    let attributes = std::collections::BTreeMap::new();
    let resource = draft_core::extension::ResourceView {
        locator_scheme: "file",
        locator_body: "src/main.ex",
        media_type: None,
        form: None,
        attributes: &attributes,
        content_size: None,
    };
    let class_id = |value: &str| draft_core::extension::NamespacedId::parse(value).unwrap();

    // --- Ownership: a class belongs to the namespace that minted it -------
    //
    // Two publishers cannot "agree" about `shared.example/source`, because
    // neither owns that namespace. Letting them would be the collision this
    // whole design exists to prevent: one publisher's rules quietly applying
    // to another's vocabulary.
    let _home = GlobalHome::new();
    let _zed = install("zed.example", "shared.example/source", "Example (zed)");
    let _alpha = install("alpha.example", "shared.example/source", "Example");

    let active = draft_extension_service::contributions::resolve().unwrap();
    let outcome = active.classes_for(&resource);
    assert!(
        outcome.assigned.is_empty(),
        "an unowned class is contributed by nobody: {:?}",
        outcome.assigned
    );
    assert!(
        outcome.collisions.is_empty(),
        "and it is refused outright rather than reported as a disagreement"
    );

    // --- Composition: two publishers, two different classes ---------------
    let _home = GlobalHome::new();
    let _text = install("zed.example", "zed.example/document", "Document");
    let _code = install("alpha.example", "alpha.example/source", "Source");

    let active = draft_extension_service::contributions::resolve().unwrap();
    let outcome = active.classes_for(&resource);
    assert_eq!(
        outcome.assigned,
        std::collections::BTreeSet::from([
            class_id("alpha.example/source"),
            class_id("zed.example/document"),
        ]),
        "different classes compose: a resource is several things at once"
    );
    assert!(
        outcome.collisions.is_empty(),
        "and neither makes the other ambiguous"
    );

    // --- Conflict: one publisher, one class, two definitions --------------
    //
    // Ownership means a class id has exactly one publisher, so a *cross*-
    // publisher dispute cannot arise. What can is a package that declares the
    // same class twice and means different things by it — and that is scoped
    // to the disputed class, never spread to the rest.
    let _home = GlobalHome::new();
    let disputed = tempfile::tempdir().unwrap();
    write_association(
        disputed.path(),
        "zed.example",
        "zed.example/source",
        "Example (zed)",
    );
    std::fs::write(
        disputed.path().join("contributions/other.json"),
        serde_json::to_vec(&serde_json::json!({
            "class_id": "zed.example/source",
            "display_name": "Example (zed)",
            // Same id, incompatible definition: a different attribute set
            // means the package does not agree with itself about what the
            // class is.
            "applies_to": {"predicate": "path_suffix", "suffix": ".ex"},
            "attributes": [{"name": "dialect", "value": "zed"}]
        }))
        .unwrap(),
    )
    .unwrap();
    let manifest_path = disputed.path().join("extension.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["contributions"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": "classification-other",
            "kind": "resource_classification",
            "path": "contributions/other.json"
        }));
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    draft_extension_service::extension::install(disputed.path()).unwrap();
    let _unrelated = install("beta.example", "beta.example/other", "Other");

    let active = draft_extension_service::contributions::resolve().unwrap();
    let outcome = active.classes_for(&resource);
    assert!(
        !outcome.assigned.contains(&class_id("zed.example/source")),
        "a disputed class is withheld rather than arbitrated"
    );
    assert_eq!(
        outcome.assigned,
        std::collections::BTreeSet::from([class_id("beta.example/other")]),
        "and the collision is scoped: an unrelated class still stands"
    );
    assert!(
        outcome
            .collisions
            .iter()
            .any(|collision| collision.class_id == class_id("zed.example/source")),
        "the disputed class is reported: {:?}",
        outcome.collisions
    );
}

/// Update planning is a local read, and presentation and execution share it.
///
/// The Console derives action eligibility from this plan, so it must not
/// refresh a source, download a package or mutate anything — deriving a read
/// model has to stay side-effect-free.
#[test]
fn update_planning_is_local_and_distinguishes_no_update_from_cannot_tell() {
    use draft_extension_service::catalog::{plan_updates, UpdateOutcome};

    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = GlobalHome::new();

    // Nothing installed: an empty plan, and nothing blocking.
    let plan = plan_updates().unwrap();
    assert!(plan.entries.is_empty());
    assert_eq!(plan.applicable().count(), 0);
    assert_eq!(plan.blocking_reason(), None);

    // A package installed directly from a path has no catalog lineage, so
    // there is genuinely nothing to update from — that is `NotApplicable`,
    // never "cannot determine".
    let directory = tempfile::tempdir().unwrap();
    write_package(directory.path(), "1.0.0", vec![]);
    draft_extension_service::extension::install(directory.path()).unwrap();

    let plan = plan_updates().unwrap();
    assert_eq!(plan.entries.len(), 1);
    assert_eq!(plan.entries[0].extension_id, "example.language");
    assert_eq!(plan.entries[0].installed_version, "1.0.0");
    assert!(
        matches!(plan.entries[0].outcome, UpdateOutcome::NotApplicable { .. }),
        "a directly installed package is not blocked, it simply has no source: {:?}",
        plan.entries[0].outcome
    );
    assert_eq!(plan.applicable().count(), 0);
    assert_eq!(
        plan.blocking_reason(),
        None,
        "nothing to do is not the same as cannot tell"
    );

    // Planning changed nothing: the package is still installed and enabled,
    // and running the plan again gives the same answer.
    let after = draft_extension_service::extension::list().unwrap();
    assert_eq!(after.len(), 1);
    assert!(after[0].enabled);
    assert_eq!(plan_updates().unwrap(), plan, "planning is repeatable");

    // And executing the plan applies nothing, because nothing is applicable.
    assert!(draft_extension_service::catalog::update_all()
        .unwrap()
        .is_empty());
    assert_eq!(
        draft_extension_service::extension::list().unwrap().len(),
        1,
        "an empty plan leaves the installation untouched"
    );
}
