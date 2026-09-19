//! The scope an external mechanism runs in.
//!
//! A contributed mechanism never runs against the project directory. Draft
//! materializes exactly the inputs the operation is entitled to into a private
//! directory outside the project — and outside `.draft/**` — writes the request
//! there, runs the command with that directory as its working directory, and
//! tears it down afterwards.
//!
//! **What this buys and what it does not.** This is an *authority, budget,
//! provenance and evidence* boundary: an operation that stays inside its scope
//! leaves an accountable record, and one that wanders outside is detectable
//! against Draft-monitored state. It is not an OS sandbox. There is no
//! filesystem jail, no network isolation and no namespace confinement, and
//! nothing here should be read as claiming otherwise.

use std::path::{Path, PathBuf};

use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil::ensure_dir;

/// The canonical subdirectory names inside a scope.
pub const INPUT_DIR: &str = "input";
pub const OUTPUT_DIR: &str = "output";
pub const REQUEST_DIR: &str = "request";
pub const METADATA_DIR: &str = "metadata";

/// The request document a command-backed mechanism reads.
pub const REQUEST_FILE: &str = "request.json";

/// A private, per-operation directory an external mechanism runs in.
///
/// Dropping the scope removes it. Teardown is deterministic rather than
/// best-effort at exit, because an abandoned scope holding materialized project
/// content is exactly the leak this design exists to avoid.
#[derive(Debug)]
pub struct RuntimeScope {
    root: PathBuf,
    /// Set once the caller has taken responsibility for the directory.
    retained: bool,
}

impl RuntimeScope {
    /// Create a scope for one operation, outside the project entirely.
    ///
    /// The location is derived from the system temporary directory, the
    /// workspace and the operation, so two concurrent operations — and two
    /// concurrent workspaces — never share one.
    pub fn create(workspace_id: &str, operation_id: &str) -> DraftResult<Self> {
        let root = Self::root_for(workspace_id, operation_id);
        if root.exists() {
            std::fs::remove_dir_all(&root).map_err(|error| {
                DraftError::storage(format!("clear stale runtime scope: {error}"))
            })?;
        }
        for directory in [INPUT_DIR, OUTPUT_DIR, REQUEST_DIR, METADATA_DIR] {
            ensure_dir(&root.join(directory))?;
        }
        Self::restrict(&root)?;
        Ok(Self {
            root,
            retained: false,
        })
    }

    /// Where a scope for this workspace and operation lives.
    pub fn root_for(workspace_id: &str, operation_id: &str) -> PathBuf {
        std::env::temp_dir()
            .join("draft")
            .join(sanitize(workspace_id))
            .join(sanitize(operation_id))
    }

    /// The directory the command runs in.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn input_dir(&self) -> PathBuf {
        self.root.join(INPUT_DIR)
    }

    pub fn output_dir(&self) -> PathBuf {
        self.root.join(OUTPUT_DIR)
    }

    pub fn request_file(&self) -> PathBuf {
        self.root.join(REQUEST_DIR).join(REQUEST_FILE)
    }

    pub fn metadata_dir(&self) -> PathBuf {
        self.root.join(METADATA_DIR)
    }

    /// Materialize one input the operation is entitled to.
    ///
    /// The name is checked rather than trusted: a mechanism cannot ask for an
    /// input that escapes its own scope.
    pub fn materialize(&self, name: &str, bytes: &[u8]) -> DraftResult<PathBuf> {
        let relative = safe_relative(name)?;
        let path = self.input_dir().join(&relative);
        if let Some(parent) = path.parent() {
            ensure_dir(parent)?;
        }
        std::fs::write(&path, bytes)
            .map_err(|error| DraftError::storage(format!("materialize {name}: {error}")))?;
        Ok(path)
    }

    /// Write the canonical request document.
    pub fn write_request(&self, request: &serde_json::Value) -> DraftResult<()> {
        let bytes = crate::support::hashing::canonical_json(request);
        std::fs::write(self.request_file(), bytes.as_bytes())
            .map_err(|error| DraftError::storage(format!("write mechanism request: {error}")))
    }

    /// Resolve a mechanism-declared working subdirectory inside this scope.
    pub fn working_directory(&self, declared: Option<&str>) -> DraftResult<PathBuf> {
        let Some(declared) = declared else {
            return Ok(self.root.clone());
        };
        let relative = safe_relative(declared)?;
        let path = self.root.join(relative);
        ensure_dir(&path)?;
        Ok(path)
    }

    /// Keep the directory after this handle is dropped.
    ///
    /// Only for the caller that has taken over cleanup — an abandoned scope is
    /// recovered by [`recover_abandoned`].
    pub fn retain(mut self) -> PathBuf {
        self.retained = true;
        self.root.clone()
    }

    /// Restrict a scope to its owner where the platform can express it.
    #[cfg(unix)]
    fn restrict(root: &Path) -> DraftResult<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| DraftError::storage(format!("restrict runtime scope: {error}")))
    }

    #[cfg(not(unix))]
    fn restrict(_root: &Path) -> DraftResult<()> {
        // Windows inherits the per-user temporary directory's access control,
        // which is already owner-only. Nothing further is claimed.
        Ok(())
    }
}

impl Drop for RuntimeScope {
    fn drop(&mut self) {
        if !self.retained {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

/// Remove runtime scopes left behind by an interrupted run.
///
/// Called at startup: a crash mid-operation must not leave materialized project
/// content in the temporary directory indefinitely.
pub fn recover_abandoned(workspace_id: &str) -> DraftResult<usize> {
    let root = std::env::temp_dir()
        .join("draft")
        .join(sanitize(workspace_id));
    if !root.exists() {
        return Ok(0);
    }
    let mut removed = 0;
    for entry in std::fs::read_dir(&root)
        .map_err(|error| DraftError::storage(format!("scan runtime scopes: {error}")))?
    {
        let path = entry
            .map_err(|error| DraftError::storage(format!("scan runtime scopes: {error}")))?
            .path();
        if path.is_dir() {
            std::fs::remove_dir_all(&path).map_err(|error| {
                DraftError::storage(format!("remove abandoned runtime scope: {error}"))
            })?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Whether a path lies inside any runtime scope.
///
/// Used by the resource layer to refuse turning runtime material into project
/// state: a scratch directory is never a Resource.
pub fn is_runtime_path(path: &Path) -> bool {
    let runtime_root = std::env::temp_dir().join("draft");
    path.starts_with(&runtime_root)
}

/// Reduce an identifier to something safe to use as a directory name.
fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

/// A relative path that cannot escape the scope it is joined to.
fn safe_relative(value: &str) -> DraftResult<PathBuf> {
    let candidate = Path::new(value);
    if candidate.is_absolute() {
        return Err(DraftError::new(
            DraftErrorKind::ProtectedResourceAccess,
            format!("runtime scope path '{value}' must be relative"),
        ));
    }
    for component in candidate.components() {
        match component {
            std::path::Component::Normal(_) => {}
            _ => {
                return Err(DraftError::new(
                    DraftErrorKind::ProtectedResourceAccess,
                    format!("runtime scope path '{value}' escapes its scope"),
                ))
            }
        }
    }
    Ok(candidate.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scope_lives_outside_the_project_and_outside_the_control_plane() {
        let scope = RuntimeScope::create("prj_scope-outside", "op_1").unwrap();
        let root = scope.root().to_path_buf();
        assert!(root.exists());
        // Not under any `.draft` directory, and not under a project.
        assert!(
            !root
                .components()
                .any(|component| component.as_os_str() == ".draft"),
            "runtime scope must never live in the control plane: {root:?}"
        );
        assert!(is_runtime_path(&root));
        assert!(root.starts_with(std::env::temp_dir()));
    }

    #[test]
    fn a_scope_is_torn_down_deterministically() {
        let root = {
            let scope = RuntimeScope::create("prj_teardown", "op_2").unwrap();
            scope.materialize("a.txt", b"payload").unwrap();
            scope.root().to_path_buf()
        };
        assert!(!root.exists(), "dropping a scope must remove it");
    }

    #[test]
    fn a_mechanism_cannot_materialize_outside_its_scope() {
        let scope = RuntimeScope::create("prj_escape", "op_3").unwrap();
        for hostile in ["../escape.txt", "a/../../escape.txt"] {
            let error = scope.materialize(hostile, b"x").unwrap_err();
            assert_eq!(
                error.kind,
                DraftErrorKind::ProtectedResourceAccess,
                "{hostile}"
            );
        }
        assert!(scope.materialize("nested/ok.txt", b"x").is_ok());
    }

    #[test]
    fn a_declared_working_directory_stays_inside_the_scope() {
        let scope = RuntimeScope::create("prj_cwd", "op_4").unwrap();
        assert_eq!(scope.working_directory(None).unwrap(), scope.root());
        let nested = scope.working_directory(Some("input")).unwrap();
        assert!(nested.starts_with(scope.root()));
        assert!(scope.working_directory(Some("/etc")).is_err());
        assert!(scope.working_directory(Some("../..")).is_err());
    }

    #[test]
    fn abandoned_scopes_are_recovered() {
        let leaked = RuntimeScope::create("prj_abandoned", "op_5")
            .unwrap()
            .retain();
        assert!(leaked.exists(), "a retained scope survives its handle");
        assert_eq!(recover_abandoned("prj_abandoned").unwrap(), 1);
        assert!(!leaked.exists());
    }

    #[test]
    fn two_operations_never_share_a_scope() {
        let first = RuntimeScope::create("prj_shared", "op_a").unwrap();
        let second = RuntimeScope::create("prj_shared", "op_b").unwrap();
        assert_ne!(first.root(), second.root());
    }
}
