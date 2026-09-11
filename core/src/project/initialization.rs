//! Crash-safe project initialization.
//!
//! ```text
//! 1. mint the ProjectId
//! 2. build the COMPLETE initial .draft state in a temporary sibling directory
//! 3. fsync every file, then fsync the temporary directory
//! 4. atomic rename  <temp>  ->  .draft
//! 5. fsync the parent directory
//! ```
//!
//! # The invariant
//!
//! > Draft never exposes a project as initialized with a missing accepted
//! > Baseline, BaselineRecord, ProjectControlState, or a half-created identity.
//!
//! Initializing in place cannot hold that. Whatever order the files are written
//! in, a crash lands between two of them and leaves a `.draft/` that exists but
//! is incomplete — and every later operation then has to decide whether it is
//! looking at a new project, a broken one, or one mid-upgrade. That check would
//! have to live in every entry point, and being absent from one of them is the
//! bug.
//!
//! Building elsewhere and renaming removes the question. The directory appears
//! atomically and complete, so "exists" and "usable" are the same fact and
//! nothing downstream needs to distinguish them.
//!
//! # The sibling directory is not a temp directory
//!
//! It is created beside `.draft`, in the project root, because `rename` is only
//! atomic within a filesystem. A staging area under the system temp directory
//! could be on a different device, where the rename silently degrades into a
//! copy — which is exactly the non-atomic behaviour being avoided.

use std::path::{Path, PathBuf};

use draft_dcg_contract::ids::ProjectId;

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil;

/// Mint a fresh project identity.
///
/// Core mints; the contract crate validates. Generating identifiers is not a
/// contract concern, and giving the portable crate a random-number dependency
/// to do it would widen its dependency surface for no verification benefit.
pub fn mint_project_id() -> ProjectId {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    ProjectId::parse(format!("prj_{}", &raw[..12]))
        .expect("a hex-suffixed prj_ identifier is always valid")
}

/// The complete initial state, staged before it becomes visible.
///
/// A caller adds every file the project needs, then commits. There is
/// deliberately no way to publish a partial state: `commit` consumes the
/// staging area, so the only thing that can become `.draft` is something that
/// was fully assembled first.
#[derive(Debug)]
pub struct StagedProject {
    staging: PathBuf,
    destination: PathBuf,
}

impl StagedProject {
    /// Begin staging a project at `root`.
    pub fn begin(root: impl AsRef<Path>) -> DraftResult<Self> {
        let root = root.as_ref();
        let destination = root.join(".draft");
        if destination.exists() {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!("{} already exists", destination.display()),
            ));
        }

        // Beside `.draft`, not under the system temp directory: rename is only
        // atomic within a filesystem, and a staging area on another device
        // would degrade into a copy.
        let staging = root.join(format!(
            ".draft-initializing-{}",
            uuid::Uuid::new_v4().simple()
        ));
        if staging.exists() {
            return Err(DraftError::new(
                DraftErrorKind::ConflictDetected,
                format!("{} already exists", staging.display()),
            ));
        }
        fsutil::ensure_dir(&staging)?;
        Ok(Self {
            staging,
            destination,
        })
    }

    /// Where a file should be written, relative to the eventual `.draft`.
    pub fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.staging.join(relative)
    }

    /// Stage one file, durably.
    pub fn write(&self, relative: impl AsRef<Path>, bytes: &[u8]) -> DraftResult<()> {
        fsutil::write_atomic(&self.path(relative), bytes)
    }

    /// Stage one JSON document, durably.
    pub fn write_json<T: serde::Serialize>(
        &self,
        relative: impl AsRef<Path>,
        value: &T,
    ) -> DraftResult<()> {
        let encoded = serde_json::to_vec_pretty(value).map_err(|error| {
            DraftError::storage(format!("cannot encode initial project state: {error}"))
        })?;
        self.write(relative, &encoded)
    }

    /// Make the staged project visible, atomically.
    ///
    /// `required` names the files that must exist first. Checking here rather
    /// than trusting the caller is the point: the invariant is that a visible
    /// project is a complete one, and a caller that forgot a file would
    /// otherwise publish exactly the half-created state this design exists to
    /// prevent.
    pub fn commit(mut self, required: &[&str]) -> DraftResult<PathBuf> {
        for relative in required {
            if !self.path(relative).exists() {
                return Err(DraftError::new(
                    DraftErrorKind::Internal,
                    format!(
                        "refusing to publish an incomplete project: '{relative}' was never \
                         staged"
                    ),
                ));
            }
        }

        // Every file is already synced by write_atomic; the directory itself
        // must be too, or the entries could still be only in the page cache
        // when the rename lands.
        fsutil::sync_directory(&self.staging)?;

        let staging = std::mem::replace(&mut self.staging, PathBuf::new());
        let destination = self.destination.clone();
        std::mem::forget(self);

        std::fs::rename(&staging, &destination).map_err(|error| {
            let _ = std::fs::remove_dir_all(&staging);
            DraftError::storage(format!("cannot publish the initialized project: {error}"))
        })?;
        if let Some(parent) = destination.parent() {
            fsutil::sync_directory(parent)?;
        }
        Ok(destination)
    }
}

impl Drop for StagedProject {
    fn drop(&mut self) {
        // An abandoned staging area is removed rather than left behind. It is
        // not a partially initialized project — nothing ever pointed at it —
        // but leaving it would make the project root look damaged.
        if !self.staging.as_os_str().is_empty() {
            let _ = std::fs::remove_dir_all(&self.staging);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUIRED: &[&str] = &["project/project.json", "project/control.json"];

    fn stage_complete(root: &Path) -> StagedProject {
        let staged = StagedProject::begin(root).unwrap();
        staged
            .write_json(
                "project/project.json",
                &serde_json::json!({"schema_version": 1}),
            )
            .unwrap();
        staged
            .write_json(
                "project/control.json",
                &serde_json::json!({"generation": 0}),
            )
            .unwrap();
        staged
    }

    #[test]
    fn a_minted_identity_is_a_valid_project_id() {
        let id = mint_project_id();
        assert!(id.as_str().starts_with("prj_"));
        ProjectId::parse(id.as_str()).unwrap();
        assert_ne!(mint_project_id(), mint_project_id());
    }

    #[test]
    fn a_complete_project_appears_atomically() {
        let root = tempfile::tempdir().unwrap();
        assert!(!root.path().join(".draft").exists());

        let draft = stage_complete(root.path()).commit(REQUIRED).unwrap();
        assert!(draft.join("project/project.json").exists());
        assert!(draft.join("project/control.json").exists());
    }

    #[test]
    fn an_incomplete_project_is_never_published() {
        // The invariant. A caller that forgot a file must not be able to
        // publish the half-created state this design exists to prevent.
        let root = tempfile::tempdir().unwrap();
        let staged = StagedProject::begin(root.path()).unwrap();
        staged
            .write_json("project/project.json", &serde_json::json!({}))
            .unwrap();

        let error = staged.commit(REQUIRED).unwrap_err();
        assert_eq!(error.kind, DraftErrorKind::Internal);
        assert!(
            !root.path().join(".draft").exists(),
            "nothing may become visible when the state is incomplete"
        );
    }

    #[test]
    fn an_abandoned_staging_area_leaves_no_trace() {
        // A crash before commit must leave a project root that looks
        // uninitialized, not damaged.
        let root = tempfile::tempdir().unwrap();
        {
            let _staged = stage_complete(root.path());
        }
        assert!(!root.path().join(".draft").exists());
        let leftovers: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(leftovers.is_empty(), "staging area was left behind");
    }

    #[test]
    fn staging_happens_beside_the_destination() {
        // rename is only atomic within a filesystem; a staging area elsewhere
        // could silently degrade into a copy.
        let root = tempfile::tempdir().unwrap();
        let staged = StagedProject::begin(root.path()).unwrap();
        assert_eq!(
            staged.path("x").parent().unwrap().parent().unwrap(),
            root.path()
        );
    }

    #[test]
    fn initializing_over_an_existing_project_is_refused() {
        let root = tempfile::tempdir().unwrap();
        stage_complete(root.path()).commit(REQUIRED).unwrap();
        assert_eq!(
            StagedProject::begin(root.path()).unwrap_err().kind,
            DraftErrorKind::ConflictDetected
        );
    }

    #[test]
    fn two_initializations_of_different_projects_do_not_collide() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        stage_complete(first.path()).commit(REQUIRED).unwrap();
        stage_complete(second.path()).commit(REQUIRED).unwrap();
        assert!(first.path().join(".draft").exists());
        assert!(second.path().join(".draft").exists());
    }

    #[test]
    fn nested_paths_are_staged_without_the_caller_creating_directories() {
        let root = tempfile::tempdir().unwrap();
        let staged = StagedProject::begin(root.path()).unwrap();
        staged
            .write("graph/baselines/records/bas_1.json", b"{}")
            .unwrap();
        assert!(staged.path("graph/baselines/records/bas_1.json").exists());
    }
}
