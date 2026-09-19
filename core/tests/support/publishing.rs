//! A project that can actually publish.
//!
//! Publication now runs Phase 1 before anything else: the trust fence, the
//! publication lease, the project's control record, its security state, the
//! grant that authorizes the effect, and the binding the route names. None of
//! that can be faked into place field by field, and a test that tried would be
//! proving something about a configuration Draft cannot reach.
//!
//! So the fixture builds a real project through the application boundary and
//! hands back the pieces a dispatch test needs.

use draft_core::app::publish::{ensure_publication, PublishRequest};
use draft_core::app::App;
use draft_core::project::Workspace;
use draft_dcg_contract::ids::PublicationId;
use draft_dcg_contract::publication::{DeliverySemantics, PublicationPurposeId};

pub struct PublishingProject {
    pub _directory: tempfile::TempDir,
    pub app: App,
    pub workspace: Workspace,
    pub root: std::path::PathBuf,
}

impl PublishingProject {
    /// A project with an accepted, promoted Baseline and publish authority.
    pub fn new() -> Self {
        static GLOBAL_HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        let home = GLOBAL_HOME.get_or_init(|| tempfile::tempdir().unwrap());
        std::env::set_var("DRAFT_GLOBAL_HOME", home.path());

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        std::fs::write(root.join("a.txt"), "hello").unwrap();

        let app = App::new();
        app.init_with_base(&root, "base change").unwrap();
        std::fs::write(
            root.join(".draft/verify.toml"),
            "schema_version = 1\n\n[[checks]]\nname = \"always\"\nenabled = true\n\n\
             [checks.command]\nprogram = \"true\"\nargs = []\n",
        )
        .unwrap();

        let project = Self {
            workspace: app.open(&root).unwrap(),
            _directory: directory,
            app,
            root,
        };
        project.promote_once();
        project.app.dcg_grant_publish(&project.root).unwrap();
        // Reopened so the workspace reflects the Baseline the promotion accepted.
        Self {
            workspace: project.app.open(&project.root).unwrap(),
            ..project
        }
    }

    /// Take one change all the way to an accepted Baseline.
    ///
    /// Publication may only ever act on a Baseline a promotion produced, so a
    /// project that has not promoted cannot publish at all.
    fn promote_once(&self) {
        let scope: Vec<String> = self
            .app
            .dcg_baseline(&self.root)
            .unwrap()
            .unwrap()
            .composition
            .keys()
            .map(ToString::to_string)
            .collect();
        let parent = self.app.dcg_baseline(&self.root).unwrap().unwrap().baseline;

        let change = self
            .app
            .dcg_open_change_pack(&self.root, "work to publish", &scope)
            .unwrap();
        std::fs::write(self.root.join("a.txt"), "published").unwrap();
        let revision = self.app.dcg_seal(&self.root, change.id.as_str()).unwrap();
        self.app
            .dcg_verify(&self.root, revision.id.as_str())
            .unwrap();
        self.app
            .dcg_assess(&self.root, revision.id.as_str(), "low", "reviewed")
            .unwrap();
        let gate = self
            .app
            .dcg_evaluate_gate(&self.root, revision.id.as_str(), &[])
            .unwrap();
        let decision = self
            .app
            .dcg_decide(&self.root, revision.id.as_str(), Some(&gate.id), true, None)
            .unwrap();
        self.app
            .dcg_promote(
                &self.root,
                change.id.as_str(),
                revision.id.as_str(),
                decision.id.as_str(),
                &gate.id,
                Some(&parent.digest().to_string()),
            )
            .unwrap();
    }

    /// A publish request for this project's accepted Baseline.
    pub fn request(&self, request_id: &str, semantics: DeliverySemantics) -> PublishRequest {
        PublishRequest {
            baseline: draft_core::dcg::baseline::current_baseline(&self.workspace.layout)
                .unwrap()
                .unwrap(),
            purpose: PublicationPurposeId::parse("draft.publish/export").unwrap(),
            semantics,
            retry_authorization: None,
            republish_intent: None,
            request_id: request_id.into(),
        }
    }

    /// The Publication a request names, created if it does not exist.
    pub fn publication(&self, request: &PublishRequest) -> PublicationId {
        ensure_publication(&self.workspace, request).unwrap().id
    }
}
