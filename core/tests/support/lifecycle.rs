//! Fixtures for the observation-lifecycle and acceptance proofs.
//!
//! Everything here is setup, never assertion. A package "installation" is
//! simulated by fixing the active contributions to exactly what a real install
//! would resolve to, so context assembly, digesting, diffing, adoption and
//! submission all run their production paths — the only thing faked is the
//! installer.

use draft_core::app::App;
use draft_core::extension::{ActiveContributions, Contributed, ExtensionContributionSource};
use draft_extension_contract::{
    ControlPolicy, PolicyPreset, RawResourcePredicate, ResourceRule, ViewRulePolicy,
};
use std::path::Path;
use std::sync::Arc;

/// One global store per test binary, so parallel binaries never share state.
pub fn global_home(tag: &str) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    let tag = tag.to_string();
    ONCE.call_once(move || {
        let root = std::env::temp_dir().join(format!("draft-{tag}-global-{}", std::process::id()));
        std::env::set_var("DRAFT_GLOBAL_HOME", root);
    });
}

#[derive(Clone)]
struct FixedContributions(Arc<ActiveContributions>);

impl ExtensionContributionSource for FixedContributions {
    fn active_contributions(&self) -> ActiveContributions {
        (*self.0).clone()
    }
}

/// Contributions equivalent to a package whose view rules exclude one subtree.
pub fn excluding(directory: &str) -> ActiveContributions {
    let mut contributions = ActiveContributions::default();
    contributions.policies.push(Contributed::new(
        "test.view.rules",
        PolicyPreset {
            view_rules: ViewRulePolicy {
                exclusions: vec![ResourceRule {
                    predicate: RawResourcePredicate::PathGlob {
                        glob: format!("{directory}/**"),
                    },
                    reason: format!("{directory} is not authored project state"),
                }],
            },
            control_policy: ControlPolicy::default(),
        },
    ));
    contributions
}

pub fn app_with(contributions: ActiveContributions) -> App {
    App::with_extension_contributions(Arc::new(FixedContributions(Arc::new(contributions))))
}

/// An initialized project with one tracked resource and one excludable subtree.
pub fn project(tag: &str) -> tempfile::TempDir {
    global_home(tag);
    let directory = tempfile::tempdir().unwrap();
    App::new().init(directory.path()).unwrap();
    std::fs::write(directory.path().join("app.txt"), "hello\n").unwrap();
    std::fs::create_dir_all(directory.path().join("cache")).unwrap();
    std::fs::write(directory.path().join("cache/blob.bin"), "cached\n").unwrap();
    directory
}

/// Observe once, so the project has an adopted context to differ from.
pub fn establish_baseline(root: &Path) -> String {
    let app = App::new();
    app.status(root).unwrap();
    app.observation_context(root).unwrap().context_digest
}

/// A minimal project with one resource and no extensions.
pub fn plain_project(tag: &str) -> tempfile::TempDir {
    global_home(tag);
    let directory = tempfile::tempdir().unwrap();
    App::new().init(directory.path()).unwrap();
    std::fs::write(directory.path().join("app.txt"), "hello\n").unwrap();
    directory
}

/// Tighten the project's risk thresholds.
///
/// A statement about what Draft will now insist on. It moves exactly one half
/// of the acceptance context — the risk thresholds — which is what makes it
/// useful for showing that the other halves, and the decisions made against
/// them, are left alone.
pub fn tighten_risk_thresholds(root: &Path) {
    std::fs::write(
        root.join(".draft/risk.toml"),
        "schema_version = 1\n\n[thresholds]\nmedium = 1\nhigh = 2\ncritical = 3\n",
    )
    .unwrap();
}
