//! Installing the official packages restores the domain behaviour Draft moved
//! out of its core.
//!
//! This is the other half of `core/tests/no_extension_capabilities.rs`. That
//! test proves Draft is a generic platform with nothing installed; this one
//! proves nothing was lost — the same classification, verification, comparison
//! and ecosystem risk weighting come back when the corresponding package is
//! installed and authorized.
//!
//! The catalog is built by the real packager from the real `/extensions/`
//! sources, signed with an ephemeral key, and installed through the ordinary
//! trusted-catalog path. Nothing here is a fixture standing in for the shipping
//! artifacts.

use draft_core::extension::{ExtensionPermission, NamespacedId, ResourceView};
use draft_core::support::common::OperationId;
use draft_extension_service::{authorization, catalog, contributions, discovery, extension};
use std::collections::{BTreeMap, BTreeSet};

#[path = "catalog_fixture.rs"]
mod catalog_fixture;
use catalog_fixture::{
    build_official_catalog, env_lock, repository_root, trust_catalog, GlobalHome,
};

#[test]
fn installing_the_official_packages_restores_the_migrated_behaviour() {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = GlobalHome::new();

    let catalog_dir = tempfile::tempdir().unwrap();
    let fingerprint = match build_official_catalog(catalog_dir.path()) {
        Ok(fingerprint) => fingerprint,
        Err(error) => panic!("could not build the official catalog: {error}"),
    };
    trust_catalog("draft-official-test", catalog_dir.path(), &fingerprint);

    // Every official package is discoverable by its own metadata.
    let found = discovery::search(&discovery::DiscoveryQuery::new("")).unwrap();
    assert_eq!(
        found.total,
        15,
        "every official package is published: {:?}",
        found
            .results
            .iter()
            .map(|entry| entry.target.id.clone())
            .collect::<Vec<_>>()
    );
    let rust = discovery::search(&discovery::DiscoveryQuery::new("rust")).unwrap();
    assert_eq!(rust.results[0].target.id, "draft.language.rust");

    // Nothing is installed until the user asks, and the id resolves exactly.
    assert!(extension::list().unwrap().is_empty());
    let source = discovery::resolve_source("draft.language.rust", None).unwrap();
    let installed = catalog::install_from_source(&source, "draft.language.rust", None).unwrap();
    assert_eq!(installed.id(), "draft.language.rust");

    // Installed but unauthorized: the classification is live, the command-backed
    // check is not. Knowledge and permission are separate, and the difference is
    // visible rather than silent.
    let before_grant = contributions::resolve().unwrap();
    let attributes = BTreeMap::new();
    let source = ResourceView {
        locator_scheme: "file",
        locator_body: "src/main.rs",
        media_type: None,
        form: None,
        attributes: &attributes,
        content_size: None,
    };
    let classes = before_grant.classes_for(&source).assigned;
    assert!(
        classes.contains(&NamespacedId::parse("draft.language.rust/source").unwrap()),
        "classification needs no permission: {classes:?}"
    );
    assert!(
        before_grant.checks_for(&source, &classes).is_empty(),
        "a command-backed check stays inert without an authorization"
    );
    assert!(
        before_grant
            .withheld
            .iter()
            .any(|withheld| withheld.extension_id == "draft.language.rust"),
        "and what was withheld is reported, so a quiet capability is not mistaken for an absent one"
    );

    // Authorizing restores the checks Draft Core used to hardcode.
    authorization::authorize(
        "draft.language.rust",
        &[ExtensionPermission::ProcessExecute],
        &OperationId::new("op_official_test"),
    )
    .unwrap();
    let after_grant = contributions::resolve().unwrap();
    let classes = after_grant.classes_for(&source).assigned;
    let checks = after_grant.checks_for(&source, &classes);
    let suite = checks
        .get(&NamespacedId::parse("draft.language.rust/suite").unwrap())
        .expect("the Rust package contributes a suite check");
    assert_eq!(
        suite
            .check
            .operation
            .executor
            .command()
            .map(|command| command.display()),
        Some("cargo test --workspace".to_string()),
        "the command Draft used to infer is back, contributed"
    );
    assert!(
        checks.contains_key(&NamespacedId::parse("draft.language.rust/toolchain").unwrap()),
        "the toolchain probe is contributed too, as an environment probe"
    );

    // The ecosystem risk rule Core no longer ships is contributed as well.
    assert!(
        after_grant.risk_rules.iter().any(|set| set
            .value
            .rules
            .iter()
            .any(|rule| rule.code.qualified() == "draft.language.rust/dependency-lockfile")),
        "the lockfile risk rule moved into the package, not away"
    );

    // And a resource is legitimately several things at once. Installing the text
    // package alongside adds a class without displacing the language one.
    let text_source = discovery::resolve_source("draft.text.document", None).unwrap();
    catalog::install_from_source(&text_source, "draft.text.document", None).unwrap();
    let composed = contributions::resolve().unwrap();
    let classes = composed.classes_for(&source).assigned;
    assert_eq!(
        classes,
        BTreeSet::from([
            NamespacedId::parse("draft.language.rust/source").unwrap(),
            NamespacedId::parse("draft.text.document/document").unwrap(),
        ]),
        "two correct classifiers of one resource are two facts, not a conflict"
    );
    assert!(
        composed.classes_for(&source).collisions.is_empty(),
        "and neither makes the other ambiguous"
    );

    // With text installed, the resource also gains a comparison — which is what
    // makes line-level review units possible at all.
    assert!(
        composed.comparison_for(&source, &classes).value().is_some(),
        "the text package explains how a document changed"
    );
}

#[test]
fn each_official_package_installs_and_contributes_what_it_declares() {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = GlobalHome::new();

    let catalog_dir = tempfile::tempdir().unwrap();
    let fingerprint = build_official_catalog(catalog_dir.path())
        .unwrap_or_else(|error| panic!("could not build the official catalog: {error}"));
    trust_catalog("draft-official-test", catalog_dir.path(), &fingerprint);

    let published: Vec<String> = discovery::search(&discovery::DiscoveryQuery::new(""))
        .unwrap()
        .results
        .into_iter()
        .map(|entry| entry.target.id)
        .collect();

    for package_id in &published {
        let source = discovery::resolve_source(package_id, None).unwrap();
        let installed = catalog::install_from_source(&source, package_id, None).unwrap();
        assert!(installed.enabled);
        assert_eq!(
            installed.provenance.catalog_source_id(),
            Some("draft-official-test")
        );
        // Provenance records the lineage an update must follow.
        assert_eq!(
            discovery::update_source(package_id, None).unwrap(),
            "draft-official-test"
        );
    }

    // Classification needs no permission at all, so it is fully live the moment
    // a package is installed.
    let active = contributions::resolve().unwrap();
    assert!(
        active.classifications.len() >= 9,
        "each language package contributes a class: {}",
        active.classifications.len()
    );
    for language in ["rust", "python", "go", "java", "c-cpp", "shell", "hcl"] {
        let class = NamespacedId::parse(&format!("draft.language.{language}/source")).unwrap();
        assert!(
            active
                .classifications
                .iter()
                .any(|rule| rule.value.class_id == class),
            "{language} contributes its class"
        );
    }
}

#[test]
fn official_provenance_alone_authorizes_nothing() {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = GlobalHome::new();

    let catalog_dir = tempfile::tempdir().unwrap();
    let fingerprint = build_official_catalog(catalog_dir.path())
        .unwrap_or_else(|error| panic!("could not build the official catalog: {error}"));
    trust_catalog("draft-official-test", catalog_dir.path(), &fingerprint);

    let installed =
        catalog::install_from_source("draft-official-test", "draft.language.rust", None).unwrap();

    // Verified through a trusted signed catalog, published by `draft`, and
    // still holding no capability. Trusted provenance is not a permission.
    assert!(installed.provenance.catalog_id().is_some());
    assert_eq!(installed.manifest.publisher, "draft");
    assert!(!authorization::authorizes(&installed, ExtensionPermission::ProcessExecute).unwrap());
    let pending = authorization::pending(&installed).unwrap().unwrap();
    assert_eq!(
        pending.missing_permissions,
        vec![ExtensionPermission::ProcessExecute]
    );

    // A package that asks for nothing has nothing outstanding, official or not.
    let declarative_only =
        catalog::install_from_source("draft-official-test", "draft.text.document", None).unwrap();
    assert!(authorization::pending(&declarative_only).unwrap().is_none());
}

#[test]
fn catalog_search_metadata_matches_the_package_manifests() {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = GlobalHome::new();

    let catalog_dir = tempfile::tempdir().unwrap();
    let fingerprint = build_official_catalog(catalog_dir.path())
        .unwrap_or_else(|error| panic!("could not build the official catalog: {error}"));
    trust_catalog("draft-official-test", catalog_dir.path(), &fingerprint);

    let packages_dir = repository_root().join("extensions/packages");
    for entry in discovery::search(&discovery::DiscoveryQuery::new(""))
        .unwrap()
        .results
    {
        let manifest_bytes =
            std::fs::read(packages_dir.join(&entry.target.id).join("extension.json")).unwrap();
        let manifest: draft_core::extension::ExtensionManifest =
            serde_json::from_slice(&manifest_bytes).unwrap();

        // Derived, never authored twice: a catalog cannot describe a package
        // differently from how the package describes itself.
        assert_eq!(entry.target.name.as_deref(), Some(manifest.name.as_str()));
        assert_eq!(entry.target.description, manifest.description);
        assert_eq!(entry.target.keywords, manifest.keywords);
        assert_eq!(entry.target.capabilities, manifest.capabilities());
        assert_eq!(entry.target.version, manifest.version);
        assert_eq!(entry.target.publisher, manifest.publisher);
    }
}
