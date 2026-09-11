//! Draft's own `file`-scheme adapter, reached through the ordinary port.
//!
//! This is Core code, not an extension: it needs no package, no attestation and
//! no grant, and its observation provenance says so. What it does *not* get is a
//! private path into Draft. Everything above [`ResourceSource`] talks to this
//! adapter exactly as it talks to a contributed one, so a capability the
//! filesystem enjoys is a capability the contract offers everybody.
//!
//! Two filesystem-specific judgements live here and nowhere else. The universe
//! is partitioned into one domain plus an incomplete domain per unreadable
//! subtree — a decision about how this adapter's world divides, not a rule Core
//! knows. And `.draft/**` is filtered structurally, ahead of every contributed
//! view rule, because Draft's control plane is not project state.

use std::fs;
use std::path::{Path, PathBuf};

use draft_extension_contract::{AdapterCapabilities, ObservationConsistency};

use crate::dcg::anchor::{RecoveryAnchor, RecoveryAnchorSet, ResourceRestorePlan};
use crate::dcg::observation::AdapterBindingId;
use crate::dcg::resource::{
    stale_observation, ContentAccess, ObservedRef, RawObservedResource, ResourceLocator,
};
use crate::dcg::snapshot::{filesystem_binding_id, Scanner};
use crate::dcg::source::{
    AnchorRequest, EnumerationOutcome, MaterializedInput, MutationOutcome, MutationPrecondition,
    MutationStep, ResourceMutationPlan, ResourceSource, ViewRules,
};
use crate::project::object_store::ObjectStore;
use crate::project::Workspace;
use crate::support::common::WorkspacePath;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::pathguard::is_draft_path;

/// The largest single read this adapter will serve into memory.
///
/// A bound rather than a convention: an operation that asks for an unbounded
/// read of an arbitrarily large resource is refused before the allocation, not
/// after it.
pub const MAX_READ_BYTES: u64 = 64 * 1024 * 1024;

/// Owns its workspace rather than borrowing one.
///
/// The registry holds adapters behind `dyn` for as long as a project is open,
/// and a borrowed adapter could not go in it — which would put Draft's own
/// observer back outside the port, reached by a branch above it. A `Workspace`
/// is an id, a root and a layout, so owning one costs nothing and keeps the
/// filesystem adapter an ordinary registry entry.
pub struct FilesystemSource {
    workspace: Workspace,
}

impl FilesystemSource {
    pub fn new(workspace: &Workspace) -> Self {
        Self {
            workspace: workspace.clone(),
        }
    }

    /// The filesystem path behind one of this adapter's locator bodies.
    ///
    /// Guarded rather than joined: a body is adapter-owned data, and this is the
    /// one place it becomes a path, so this is where traversal is refused.
    fn resolve(&self, locator: &ResourceLocator) -> DraftResult<PathBuf> {
        if locator.scheme != crate::support::predicate::FILE_SCHEME {
            return Err(DraftError::invalid_config(format!(
                "the filesystem adapter was handed a '{}' locator",
                locator.scheme
            )));
        }
        safe_workspace_path(&self.workspace.root, &WorkspacePath::new(&locator.body))
    }

    /// Recompute a locator's state digest from what is on disk right now.
    fn restate(&self, locator: &ResourceLocator) -> DraftResult<String> {
        let path = self.resolve(locator)?;
        Scanner::new(&self.workspace, Vec::new())?.restate(&path, locator)
    }

    /// Refuse to serve content from a generation other than the observed one.
    ///
    /// This adapter declares `BestEffortGeneration`: its token can alias, so the
    /// digest is what is actually compared. Reading first and checking after
    /// would be too late — the bytes would already be attributed to a state
    /// they did not come from.
    fn fence(&self, observed: &ObservedRef) -> DraftResult<PathBuf> {
        let path = self.resolve(&observed.locator)?;
        let current = self.restate(&observed.locator)?;
        if current != observed.expected_state_digest {
            return Err(stale_observation(
                &observed.locator,
                &observed.expected_state_digest,
                &current,
            ));
        }
        Ok(path)
    }

    /// Check every precondition immediately before acting on the plan.
    fn check_preconditions(&self, plan: &ResourceMutationPlan) -> DraftResult<()> {
        for precondition in &plan.preconditions {
            match precondition {
                MutationPrecondition::StateEquals(observed)
                | MutationPrecondition::ParentStateEquals(observed) => {
                    self.fence(observed)?;
                }
                MutationPrecondition::MustNotExist(locator)
                | MutationPrecondition::DestinationAvailable(locator) => {
                    let path = self.resolve(locator)?;
                    if path.exists() || path.is_symlink() {
                        return Err(DraftError::new(
                            DraftErrorKind::ConflictDetected,
                            format!(
                                "{locator} already exists; the operation expected it to be free"
                            ),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

impl ResourceSource for FilesystemSource {
    fn scheme(&self) -> &str {
        crate::support::predicate::FILE_SCHEME
    }

    fn binding_id(&self) -> AdapterBindingId {
        filesystem_binding_id()
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            // The token is device/inode/size/mtime, which can alias within a
            // timestamp tick. Declaring the weaker guarantee is what makes Draft
            // revalidate rather than trust it.
            observation_consistency: ObservationConsistency::BestEffortGeneration,
            supports_ranged_read: true,
            supports_mutation: true,
            // A path is not an identity. This adapter cannot prove a file it did
            // not move is the same file, so a relocation it did not perform is
            // conservatively a removal plus an addition.
            asserts_external_identity: false,
        }
    }

    fn enumerate(&self, rules: &ViewRules) -> DraftResult<EnumerationOutcome> {
        Scanner::new(&self.workspace, rules.exclusions.clone())?.enumerate()
    }

    fn describe(&self, locator: &ResourceLocator) -> DraftResult<RawObservedResource> {
        let path = self.resolve(locator)?;
        if !path.exists() && !path.is_symlink() {
            return Err(DraftError::not_found(format!("{locator} does not exist")));
        }
        Scanner::new(&self.workspace, Vec::new())?.describe_one(&path, locator)
    }

    fn content_access(&self, observed: &ObservedRef) -> DraftResult<ContentAccess> {
        let path = self.fence(observed)?;
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            return Ok(ContentAccess::None);
        }
        Ok(ContentAccess::Ranged {
            length: metadata.len(),
            media_type: None,
        })
    }

    fn read_range(&self, observed: &ObservedRef, offset: u64, length: u64) -> DraftResult<Vec<u8>> {
        if length > MAX_READ_BYTES {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!("a {length}-byte read exceeds the {MAX_READ_BYTES}-byte access bound"),
            ));
        }
        let path = self.fence(observed)?;
        let bytes = fs::read(&path)?;
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let end = start
            .saturating_add(usize::try_from(length).unwrap_or(usize::MAX))
            .min(bytes.len());
        Ok(bytes[start..end].to_vec())
    }

    fn materialize(
        &self,
        observed: &ObservedRef,
        scope: &crate::support::runtime_scope::RuntimeScope,
    ) -> DraftResult<MaterializedInput> {
        let path = self.fence(observed)?;
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.len() > MAX_READ_BYTES {
            return Err(DraftError::new(
                DraftErrorKind::Validation,
                format!(
                    "{} is {} bytes, over the {MAX_READ_BYTES}-byte materialization bound",
                    observed.locator,
                    metadata.len()
                ),
            ));
        }
        let bytes = fs::read(&path)?;
        // Named by the resource id, not by the locator body: the body is this
        // adapter's, and a runtime input name is Draft's.
        let name = materialized_name(observed);
        scope.materialize(&name, &bytes)?;
        Ok(MaterializedInput {
            name,
            length: bytes.len() as u64,
        })
    }

    fn mutate(&self, plan: &ResourceMutationPlan) -> DraftResult<MutationOutcome> {
        self.check_preconditions(plan)?;
        let mut changed = Vec::new();
        for step in &plan.steps {
            match step {
                MutationStep::SetContent { locator, content } => {
                    let dest = self.resolve(locator)?;
                    if let Some(parent) = dest.parent() {
                        crate::support::fsutil::ensure_dir(parent)?;
                    }
                    crate::support::fsutil::write_atomic(&dest, content)?;
                    changed.push(locator.clone());
                }
                MutationStep::CreateCollection { locator } => {
                    crate::support::fsutil::ensure_dir(&self.resolve(locator)?)?;
                    changed.push(locator.clone());
                }
                MutationStep::Relocate { from, to } => {
                    let source = self.resolve(from)?;
                    let dest = self.resolve(to)?;
                    if let Some(parent) = dest.parent() {
                        crate::support::fsutil::ensure_dir(parent)?;
                    }
                    fs::rename(&source, &dest).map_err(|error| {
                        DraftError::storage(format!("cannot relocate {from} to {to}: {error}"))
                    })?;
                    changed.push(from.clone());
                    changed.push(to.clone());
                }
                MutationStep::Remove { locator, recursive } => {
                    let path = self.resolve(locator)?;
                    if path.is_dir() && *recursive {
                        fs::remove_dir_all(&path).map_err(|error| {
                            DraftError::storage(format!("cannot remove {locator}: {error}"))
                        })?;
                    } else if path.is_file() || path.is_symlink() {
                        fs::remove_file(&path).map_err(|error| {
                            DraftError::storage(format!("cannot remove {locator}: {error}"))
                        })?;
                    }
                    changed.push(locator.clone());
                }
            }
        }
        changed.sort();
        changed.dedup();
        Ok(MutationOutcome {
            resources_changed: changed,
        })
    }

    fn capture_anchor(
        &self,
        observed: &RawObservedResource,
        request: &AnchorRequest,
    ) -> DraftResult<Option<RecoveryAnchor>> {
        use crate::dcg::anchor::{
            AnchorCapture, CanonicalObjectRef, FencingEvidence, RecoveryMaterial,
        };
        let Some(content_digest) = &observed.state.content_digest else {
            return Ok(None);
        };
        let store = ObjectStore::new(self.workspace.layout.clone());

        // Revalidate around the capture. A capture from a generation other than
        // the one being anchored would be evidence about a different moment, and
        // nothing downstream could tell.
        let after = match self.restate(&observed.state.locator) {
            Ok(digest) => digest,
            // Moved on or vanished between observation and capture. No anchor:
            // a missing one is honest, a mismatched one is not.
            Err(_) => return Ok(None),
        };
        if after != observed.state.state_digest {
            return Ok(None);
        }

        // Bytes alone would restore the content and not the state. Everything
        // else that participates in this adapter's state digest is retained
        // beside them, so the post-restore comparison can actually succeed.
        let restore_state = serde_json::json!({
            "form": observed.state.form,
            "attributes": observed.state.attributes,
            "locator": observed.state.locator,
        });
        let restore_bytes = crate::support::hashing::canonical_json(&restore_state);
        let restore_digest = store.put_bytes(restore_bytes.as_bytes())?;

        Ok(Some(
            RecoveryAnchor {
                schema_version: crate::contracts::current_version(
                    crate::contracts::ContractId::RecoveryAnchorSet,
                ),
                resource_id: observed.state.resource_id.clone(),
                target_state_digest: observed.state.state_digest.clone(),
                target_locator: observed.state.locator.clone(),
                adapter_binding_id: filesystem_binding_id(),
                recovery_material: RecoveryMaterial::ContentObject {
                    object_digest: content_digest.clone(),
                    length: observed.state.content_size.unwrap_or_default(),
                    restore_state: Some(CanonicalObjectRef {
                        object_digest: restore_digest,
                        length: restore_bytes.len() as u64,
                    }),
                },
                capture: AnchorCapture {
                    observed_state_digest: observed.state.state_digest.clone(),
                    observation_run_id: request.observation_run_id.clone(),
                    fenced_with: FencingEvidence::DigestRevalidated {
                        before: observed.state.state_digest.clone(),
                        after,
                    },
                    captured_at: crate::support::common::now(),
                },
                // Core's own observer: no producer, no attestation, no grant.
                producer: None,
                recorded_at: crate::support::common::now(),
                anchor_digest: String::new(),
            }
            .seal()?,
        ))
    }

    fn restore(
        &self,
        plan: &ResourceRestorePlan,
        anchors: &RecoveryAnchorSet,
    ) -> DraftResult<MutationOutcome> {
        use crate::dcg::anchor::RecoveryMaterial;
        let store = ObjectStore::new(self.workspace.layout.clone());
        let mut changed = Vec::new();

        // Absence first, so restoring into a locator a stale resource occupies
        // is not blocked by it.
        for absence in &plan.absence_targets {
            if absence.current_locator.scheme != self.scheme() {
                continue;
            }
            if is_draft_path(&absence.current_locator.body) {
                continue;
            }
            let path = self.resolve(&absence.current_locator)?;
            if path.is_file() || path.is_symlink() {
                fs::remove_file(&path).map_err(|error| {
                    DraftError::storage(format!(
                        "cannot remove {}: {error}",
                        absence.current_locator
                    ))
                })?;
                changed.push(absence.current_locator.clone());
            }
        }

        for restore in &plan.restore_targets {
            if restore.target_locator.scheme != self.scheme() {
                continue;
            }
            if is_draft_path(&restore.target_locator.body) {
                continue;
            }
            let Some(anchor) = anchors.anchor_for(&restore.resource_id) else {
                continue;
            };
            let RecoveryMaterial::ContentObject {
                object_digest,
                restore_state,
                ..
            } = &anchor.recovery_material
            else {
                // Another adapter's material. Its own restore mechanism handles
                // it; this one does not guess at an opaque payload.
                continue;
            };
            // The destination comes from the target snapshot, never from where
            // the resource happens to sit now.
            let dest = self.resolve(&restore.target_locator)?;
            if let Some(parent) = dest.parent() {
                crate::support::fsutil::ensure_dir(parent)?;
            }
            crate::support::fsutil::write_atomic(&dest, &store.get_bytes(object_digest)?)?;
            if let Some(state_ref) = restore_state {
                apply_restore_state(&dest, &store.get_bytes(&state_ref.object_digest)?)?;
            }
            changed.push(restore.target_locator.clone());
        }

        changed.sort();
        changed.dedup();
        Ok(MutationOutcome {
            resources_changed: changed,
        })
    }
}

/// The name a materialized input is offered under inside a runtime scope.
fn materialized_name(observed: &ObservedRef) -> String {
    let sanitized: String = observed
        .resource_id
        .as_str()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '.' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect();
    format!("input-{sanitized}")
}

/// Put back the properties that are part of a resource's state but not its
/// bytes.
///
/// Without this a restored file would carry the right content and the wrong
/// mode, and the post-restore digest comparison would fail for a reason that
/// looked like corruption.
pub(crate) fn apply_restore_state(dest: &Path, document: &[u8]) -> DraftResult<()> {
    let state: serde_json::Value = serde_json::from_slice(document)
        .map_err(|error| DraftError::storage(format!("corrupt restore state: {error}")))?;
    let executable = state
        .get("attributes")
        .and_then(|attributes| attributes.get("file.executable"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(dest, std::fs::Permissions::from_mode(mode)).map_err(|error| {
            DraftError::storage(format!(
                "cannot restore mode of {}: {error}",
                dest.display()
            ))
        })?;
    }
    #[cfg(not(unix))]
    {
        // Windows has no executable bit, and this observer records none there,
        // so there is nothing to put back.
        let _ = (dest, executable);
    }
    Ok(())
}

/// Turn one of this adapter's locator bodies into a path, or refuse.
///
/// Traversal, absolute paths, the control plane and Windows drive syntax are
/// all rejected here rather than anywhere upstream, because this is the only
/// place a `file` body is interpreted at all.
pub(crate) fn safe_workspace_path(root: &Path, rel: &WorkspacePath) -> DraftResult<PathBuf> {
    let body = rel.as_str();
    if body.is_empty()
        || body.starts_with('/')
        || body.contains('\0')
        || body.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || (cfg!(windows) && (part.contains(':') || part.contains('\\')))
        })
        || is_draft_path(body)
    {
        return Err(DraftError::storage(format!(
            "unsafe workspace path '{body}'"
        )));
    }
    Ok(root.join(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_control_plane_is_unreachable_through_a_locator() {
        let root = Path::new("/tmp/project");
        // Not a convention enforced upstream: the adapter that owns `file`
        // bodies refuses these itself, so no caller can route around it.
        for hostile in [
            ".draft/config.toml",
            "../escape",
            "/etc/passwd",
            "nested/../../out",
            "",
        ] {
            assert!(
                safe_workspace_path(root, &WorkspacePath::new(hostile)).is_err(),
                "{hostile} must be refused"
            );
        }
        assert!(safe_workspace_path(root, &WorkspacePath::new("src/main.rs")).is_ok());
    }

    #[test]
    fn a_materialized_input_is_named_by_draft_not_by_the_body() {
        let observed = ObservedRef {
            resource_id: crate::dcg::resource::resource_id_for_locator("file:src/a b/c.rs"),
            locator: ResourceLocator::file("src/a b/c.rs"),
            expected_state_digest: "d".into(),
            observation_token: crate::dcg::resource::ObservationToken("t".into()),
        };
        let name = materialized_name(&observed);
        // Nothing path-shaped survives: a runtime input name is Draft's, and a
        // body that happened to contain separators must not create directories.
        assert!(!name.contains('/'));
        assert!(!name.contains(' '));
        assert!(name.starts_with("input-"));
    }
}
