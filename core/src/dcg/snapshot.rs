//! Draft's built-in filesystem observer.
//!
//! This is one resource adapter among however many are installed, and it happens
//! to be the one Core provides. It enumerates `file`-scheme resources, computes
//! a deterministic state digest for each, reports which coverage domains it
//! established completely, and records a gap wherever it could not.
//!
//! Two properties matter more than the walk itself:
//!
//! * **A directory it cannot read becomes a gap, not silence.** Returning fewer
//!   entries would make the next comparison read the absence as deletions.
//! * **`.draft/**` can never be observed.** Draft's own control plane is not
//!   project state, and the exclusion is applied after every contributed view
//!   rule so nothing can re-enable it.
//!
//! Coverage partitioning is this adapter's own business: it uses the workspace
//! root plus each top-level collection, so an unreadable subtree costs coverage
//! only there. Core never learns what those domains mean.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::dcg::observation::{
    AdapterBindingId, CoverageDomainRef, CoverageStatus, ObservationCoverage, ObservationGap,
    ObservationGapKind, ResourceCoverageMembership, SnapshotObservationMap,
};
use crate::dcg::resource::{
    filesystem_state_digest, ObservationToken, RawObservedResource, RawResourceState, ResourceId,
    ResourceLocator, Untrackable,
};
use crate::dcg::state::{ResourceChangeSummary, Snapshot, WorkspaceStatus};
use crate::project::object_store::ObjectStore;
use crate::project::Workspace;
use crate::support::actor::ActorRef;
use crate::support::common::{now, SnapshotId, WorkspacePath};
use crate::support::error::{DraftError, DraftResult};
use crate::support::fsutil::write_json;
use crate::support::pathguard;
use draft_extension_contract::ResourceForm;

/// The binding id Draft's own filesystem observer runs under.
pub const FILESYSTEM_BINDING: &str = "core.filesystem";

/// The Core implementation revision of this observer.
///
/// Committed to by the observation context, so changing how this adapter
/// partitions coverage or computes state cannot silently reuse a context taken
/// under the old behaviour.
pub const FILESYSTEM_OBSERVER_REVISION: u32 = 1;

/// The coverage domain covering the workspace root itself.
pub const ROOT_DOMAIN: &str = "root";

pub(crate) fn filesystem_binding_id() -> AdapterBindingId {
    AdapterBindingId(FILESYSTEM_BINDING.to_string())
}

pub(crate) fn domain(local: &str) -> CoverageDomainRef {
    CoverageDomainRef::new(filesystem_binding_id(), local)
}

pub(crate) use crate::dcg::source::EnumerationOutcome;

pub(crate) struct Scanner<'a> {
    workspace: &'a Workspace,
    ignore: IgnoreMatcher,
    /// Contributed view rules: what is not part of the observed universe at
    /// all.
    ///
    /// Draft contributes none. That another tool's control directory, or a
    /// dependency cache, is not authored project state is a judgement about the
    /// software domain, and it arrives from the package that holds that
    /// judgement. Because these rules decide what a snapshot even contains,
    /// they participate in the observation context — changing them is a
    /// re-observation, not a project change.
    view_rules: Vec<draft_extension_contract::ResourceRule>,
}

impl<'a> Scanner<'a> {
    pub(crate) fn new(
        workspace: &'a Workspace,
        view_rules: Vec<draft_extension_contract::ResourceRule>,
    ) -> DraftResult<Self> {
        Ok(Self {
            workspace,
            ignore: IgnoreMatcher::load(&workspace.layout.ignore_file())?,
            view_rules,
        })
    }

    /// Whether a contributed view rule excludes this resource from the observed
    /// universe.
    fn excluded_by_view_rules(&self, state: &RawResourceState) -> bool {
        if self.view_rules.is_empty() {
            return false;
        }
        let view = crate::support::predicate::ResourceView {
            locator_scheme: state.locator.scheme.as_str(),
            locator_body: state.locator.body.as_str(),
            media_type: state.media_type.as_deref(),
            form: state.form,
            attributes: &state.attributes,
            content_size: state.content_size,
        };
        self.view_rules
            .iter()
            .any(|rule| crate::support::predicate::matches_raw(&rule.predicate, &view))
    }

    /// Observe every `file`-scheme resource this adapter can establish.
    ///
    /// **The coverage partition.** On a clean scan this adapter declares one
    /// domain — its whole universe — and every resource belongs to it. That is
    /// the honest statement: "I enumerated everything reachable from the project
    /// root, and this is what was there." It is also what makes a first addition
    /// provable: a resource in a directory that did not exist at base time is
    /// `Added` because the base scan covered the universe that would have
    /// contained it, not because Core reasoned about paths.
    ///
    /// When part of the tree cannot be read, that subtree becomes its own
    /// `Incomplete` domain *and* the universe is marked incomplete too. Both are
    /// true, and the second is what matters: while some of the tree is unseen,
    /// nothing can be proved absent anywhere, because it might be in the part
    /// Draft could not look at. Content comparisons for resources that *were*
    /// seen are unaffected.
    pub(crate) fn enumerate(&self) -> DraftResult<EnumerationOutcome> {
        let store = ObjectStore::new(self.workspace.layout.clone());
        let mut outcome = EnumerationOutcome::default();
        let mut incomplete: BTreeMap<String, Vec<ObservationGap>> = BTreeMap::new();
        let mut domains: Vec<String> = vec![ROOT_DOMAIN.to_string()];

        for local in self.top_level_domains()? {
            domains.push(local);
        }

        for local in &domains {
            let root = self.domain_root(local);
            let mut walker = |path: &Path| -> DraftResult<()> {
                let relative = relative_path(&self.workspace.root, path)?;
                // Draft's control plane is not project state, and this filter
                // runs regardless of any contributed view rule.
                if pathguard::is_draft_path(relative.as_str()) {
                    return Ok(());
                }
                if self.ignore.is_ignored(relative.as_str()) {
                    outcome.excluded_count += 1;
                    return Ok(());
                }
                if path.is_dir() {
                    return Ok(());
                }
                // Only the walk that owns this path records it, so no resource
                // ends up with two memberships. Membership itself is always the
                // universe domain — see the note on `enumerate`.
                if self.owning_walk(relative.as_str()) != *local {
                    return Ok(());
                }
                match self.observe(path, &relative, &store, ROOT_DOMAIN) {
                    // A contributed view rule removes the resource from the
                    // observed universe entirely: it is not present, not
                    // ignored, and not a gap. Draft never saw it, and the
                    // context digest records the semantics that decided so.
                    Ok(observed) if self.excluded_by_view_rules(&observed.state) => {}
                    Ok(observed) => outcome.resources.push(observed),
                    Err(error) => outcome.untrackable.push(Untrackable {
                        locator: ResourceLocator::file(relative.as_str()),
                        reason: error.to_string(),
                    }),
                }
                Ok(())
            };
            match walk_dir(&root, &mut walker) {
                Ok(()) => {}
                Err(error) => {
                    // The subtree could not be enumerated. Recording a gap keeps
                    // "we could not look" distinct from "there was nothing".
                    let gap = ObservationGap::new(
                        gap_kind_for(&error),
                        stable_code_for(&error),
                        vec![domain(local)],
                        Some(filesystem_binding_id()),
                        error.to_string(),
                    );
                    incomplete.entry(local.clone()).or_default().push(gap);
                }
            }
        }

        // Every unreadable subtree is its own incomplete domain, so a reader
        // can see exactly what was not looked at.
        let mut universe_gaps: Vec<crate::dcg::observation::ObservationGapId> = Vec::new();
        for (local, gaps) in incomplete {
            if local == ROOT_DOMAIN {
                universe_gaps.extend(gaps.iter().map(|gap| gap.gap_id.clone()));
            } else {
                universe_gaps.extend(gaps.iter().map(|gap| gap.gap_id.clone()));
                outcome.coverage.push(ObservationCoverage {
                    domain: domain(&local),
                    status: CoverageStatus::Incomplete {
                        gap_ids: gaps.iter().map(|gap| gap.gap_id.clone()).collect(),
                    },
                });
            }
            outcome.gaps.extend(gaps);
        }

        // The universe domain, which every observed resource belongs to. It is
        // complete only when the whole tree was read: while any part is unseen,
        // absence cannot be proved anywhere in it.
        outcome.coverage.push(ObservationCoverage {
            domain: domain(ROOT_DOMAIN),
            status: if universe_gaps.is_empty() {
                CoverageStatus::Complete
            } else {
                CoverageStatus::Incomplete {
                    gap_ids: universe_gaps,
                }
            },
        });
        Ok(outcome)
    }

    /// The subtrees this adapter walks separately, so a failure in one can be
    /// reported precisely rather than blanking the whole scan.
    ///
    /// A walk boundary, not a coverage domain: every resource found by any of
    /// these walks belongs to the one universe domain.
    fn top_level_domains(&self) -> DraftResult<Vec<String>> {
        let mut domains = Vec::new();
        let entries = match fs::read_dir(&self.workspace.root) {
            Ok(entries) => entries,
            Err(_) => return Ok(domains),
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".draft" {
                continue;
            }
            if entry.path().is_dir() {
                domains.push(name);
            }
        }
        domains.sort();
        Ok(domains)
    }

    fn domain_root(&self, local: &str) -> std::path::PathBuf {
        if local == ROOT_DOMAIN {
            self.workspace.root.clone()
        } else {
            self.workspace.root.join(local)
        }
    }

    /// Recompute a resource's state digest from what is on disk right now.
    ///
    /// Used to revalidate around a bounded access: this adapter's fencing token
    /// can alias, so the digest is the thing actually compared.
    pub(crate) fn restate(&self, path: &Path, locator: &ResourceLocator) -> DraftResult<String> {
        let metadata = fs::symlink_metadata(path)?;
        let (form, content_digest, symlink_target) = if metadata.is_symlink() {
            let target = fs::read_link(path)?.to_string_lossy().into_owned();
            let digest = crate::support::hashing::blake3_hex(target.as_bytes());
            (
                ResourceForm::Reference,
                Some(format!("b3:{digest}")),
                Some(target),
            )
        } else {
            let data = fs::read(path)?;
            let digest = crate::support::hashing::blake3_hex(&data);
            (ResourceForm::Bytes, Some(format!("b3:{digest}")), None)
        };
        Ok(filesystem_state_digest(
            locator,
            Some(form),
            content_digest.as_deref(),
            executable(&metadata),
            symlink_target.as_deref(),
        ))
    }

    /// Which walk records a given path, so no resource is observed twice.
    fn owning_walk(&self, relative: &str) -> String {
        match relative.split_once('/') {
            Some((head, _)) => head.to_string(),
            None => ROOT_DOMAIN.to_string(),
        }
    }

    /// Observe exactly one resource the caller has already resolved.
    ///
    /// The single-resource counterpart of [`Scanner::enumerate`], sharing its
    /// observation logic so a resource cannot be described one way in a scan and
    /// another way on its own.
    pub(crate) fn describe_one(
        &self,
        path: &Path,
        locator: &ResourceLocator,
    ) -> DraftResult<RawObservedResource> {
        let store = ObjectStore::new(self.workspace.layout.clone());
        self.observe(
            path,
            &WorkspacePath::new(&locator.body),
            &store,
            ROOT_DOMAIN,
        )
    }

    fn observe(
        &self,
        path: &Path,
        relative: &WorkspacePath,
        store: &ObjectStore,
        local: &str,
    ) -> DraftResult<RawObservedResource> {
        let metadata = fs::symlink_metadata(path)?;
        let locator = ResourceLocator::file(relative.as_str());
        let (form, content_digest, content_size, symlink_target) = if metadata.is_symlink() {
            let target = fs::read_link(path)
                .map_err(|error| {
                    DraftError::storage(format!(
                        "cannot read link target {}: {error}",
                        path.display()
                    ))
                })?
                .to_string_lossy()
                .into_owned();
            (
                ResourceForm::Reference,
                Some(store.put_bytes(target.as_bytes())?),
                target.len() as u64,
                Some(target),
            )
        } else {
            let data = fs::read(path)?;
            let size = data.len() as u64;
            (
                ResourceForm::Bytes,
                Some(store.put_bytes(&data)?),
                size,
                None,
            )
        };
        let is_executable = executable(&metadata);
        let state_digest = filesystem_state_digest(
            &locator,
            Some(form),
            content_digest.as_deref(),
            is_executable,
            symlink_target.as_deref(),
        );
        let mut attributes = BTreeMap::new();
        if is_executable {
            attributes.insert(
                "file.executable".to_string(),
                draft_extension_contract::AttributeValue::Boolean(true),
            );
        }
        Ok(RawObservedResource {
            state: RawResourceState {
                // Locator-stable identity: this adapter cannot prove continuity
                // across a move it did not perform, so a relocated file is a
                // different resource until something asserts otherwise.
                resource_id: crate::dcg::resource::resource_id_for_locator(&format!(
                    "file:{}",
                    relative.as_str()
                )),
                locator,
                form: Some(form),
                media_type: None,
                attributes,
                state_digest,
                content_digest,
                metadata_digest: None,
                content_size: Some(content_size),
            },
            observation_token: fencing_token(&metadata),
            coverage_domain: domain(local),
        })
    }
}

/// A best-effort generation token.
///
/// Device, inode, size and modification time together change for almost every
/// real edit, but they can alias — a same-size write within one timestamp tick
/// is the classic case. This adapter therefore declares
/// `BestEffortGeneration`, and Draft revalidates the state digest around
/// content access rather than trusting the token alone.
fn fencing_token(metadata: &fs::Metadata) -> ObservationToken {
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}:{}", metadata.dev(), metadata.ino(), metadata.size())
    };
    #[cfg(not(unix))]
    let identity = format!("{}", metadata.len());
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    ObservationToken(format!("{identity}:{modified}"))
}

fn gap_kind_for(error: &DraftError) -> ObservationGapKind {
    if error.to_string().to_lowercase().contains("permission") {
        ObservationGapKind::PermissionDenied
    } else {
        ObservationGapKind::EnumerationFailed
    }
}

fn stable_code_for(error: &DraftError) -> &'static str {
    match gap_kind_for(error) {
        ObservationGapKind::PermissionDenied => "filesystem.permission_denied",
        _ => "filesystem.enumeration_failed",
    }
}

fn summary_aspects(
    before: &RawResourceState,
    after: &RawResourceState,
) -> Vec<crate::dcg::resource::ChangeAspect> {
    use crate::dcg::resource::ChangeAspect;
    let mut aspects = Vec::new();
    if before.locator != after.locator {
        aspects.push(ChangeAspect::Relocated);
    }
    if before.content_digest != after.content_digest || before.content_size != after.content_size {
        aspects.push(ChangeAspect::ContentChanged);
    }
    if before.form != after.form {
        aspects.push(ChangeAspect::FormChanged);
    }
    if before.attributes != after.attributes {
        aspects.push(ChangeAspect::AttributesChanged);
    }
    if aspects.is_empty() {
        aspects.push(ChangeAspect::MetadataChanged);
    }
    aspects
}

#[derive(Debug, Clone)]
pub(crate) struct IgnoreMatcher {
    patterns: Vec<String>,
}

impl IgnoreMatcher {
    pub(crate) fn load(path: &Path) -> DraftResult<Self> {
        Ok(Self {
            patterns: read_ignore_lines(path)?,
        })
    }

    pub(crate) fn is_ignored(&self, path: &str) -> bool {
        // `.draft/` is excluded structurally elsewhere; an ignore file can only
        // ever add to what is skipped, never subtract from it.
        self.patterns
            .iter()
            .any(|pattern| pattern_match(pattern, path))
    }
}

/// Compare the live workspace against the last authoritative observation.
pub(crate) fn workspace_status(
    workspace: &Workspace,
    observed: &EnumerationOutcome,
) -> DraftResult<WorkspaceStatus> {
    let previous = latest_snapshot(workspace)?;
    let previous_by_id: BTreeMap<ResourceId, RawResourceState> = previous
        .as_ref()
        .map(|snapshot| {
            snapshot
                .resources
                .iter()
                .map(|resource| (resource.resource_id.clone(), resource.clone()))
                .collect()
        })
        .unwrap_or_default();

    let mut changes = Vec::new();
    let current_by_id: BTreeMap<ResourceId, RawResourceState> = observed
        .resources
        .iter()
        .map(|observed| (observed.state.resource_id.clone(), observed.state.clone()))
        .collect();
    for (resource_id, after) in &current_by_id {
        match previous_by_id.get(resource_id) {
            Some(before) if before.state_digest == after.state_digest => {}
            Some(before) => changes.push(ResourceChangeSummary {
                locator: after.locator.clone(),
                aspects: summary_aspects(before, after),
                before_state_digest: Some(before.state_digest.clone()),
                after_state_digest: Some(after.state_digest.clone()),
            }),
            None => changes.push(ResourceChangeSummary {
                locator: after.locator.clone(),
                aspects: vec![crate::dcg::resource::ChangeAspect::Added],
                before_state_digest: None,
                after_state_digest: Some(after.state_digest.clone()),
            }),
        }
    }
    // Absence is only reported where the live enumeration actually covered
    // the resource's domain; anywhere else it is unknown, not gone.
    for (resource_id, before) in &previous_by_id {
        if current_by_id.contains_key(resource_id) {
            continue;
        }
        let covered = previous
            .as_ref()
            .and_then(|snapshot| snapshot.observation_map.domain_of(resource_id))
            .is_some_and(|domain| {
                observed
                    .coverage
                    .iter()
                    .any(|coverage| &coverage.domain == domain && coverage.status.is_complete())
            });
        if covered {
            changes.push(ResourceChangeSummary {
                locator: before.locator.clone(),
                aspects: vec![crate::dcg::resource::ChangeAspect::Removed],
                before_state_digest: Some(before.state_digest.clone()),
                after_state_digest: None,
            });
        }
    }

    Ok(WorkspaceStatus {
        workspace_id: workspace.workspace_id.clone(),
        root_path: workspace.root.display().to_string(),
        scanned_at: now(),
        ignored_count: observed.excluded_count,
        has_draft_dir_violation: false,
        changes,
    })
}

/// Builds and persists authoritative observations.
///
/// Everything filesystem-specific — how the universe divides into coverage
/// domains, what a state digest is made of, what can be put back — belongs to
/// the adapter and is reached through [`ResourceSource`]. What stays here is
/// what would be identical for any adapter: assembling the snapshot, sealing
/// its identity, recording who observed and grouping the anchors. Draft's own
/// observer gets no private path, so a capability the filesystem enjoys is one
/// the port offers everybody.
pub(crate) struct Snapshotter<'a> {
    workspace: &'a Workspace,
    source: crate::dcg::filesystem_source::FilesystemSource,
    view_rules: crate::dcg::source::ViewRules,
}

impl<'a> Snapshotter<'a> {
    pub(crate) fn new(
        workspace: &'a Workspace,
        view_rules: Vec<draft_extension_contract::ResourceRule>,
    ) -> DraftResult<Self> {
        Ok(Self {
            workspace,
            source: crate::dcg::filesystem_source::FilesystemSource::new(workspace),
            view_rules: crate::dcg::source::ViewRules {
                exclusions: view_rules,
            },
        })
    }

    /// Observe the project and persist everything that observation produced.
    ///
    /// Returns the exact provenance record alongside the snapshot. A caller that
    /// needs to bind "which observation established this" must be handed the
    /// answer here, at the only moment it is unambiguous — resolving it later
    /// from the store would mean choosing between records, and any rule for
    /// choosing is a rule for silently changing what a Change claims.
    pub(crate) fn create_snapshot(
        &self,
        actor: ActorRef,
        observation_context_digest: &str,
    ) -> DraftResult<(Snapshot, String)> {
        let started_at = now();
        let observed =
            crate::dcg::source::ResourceSource::enumerate(&self.source, &self.view_rules)?;
        let mut resources = Vec::new();
        let mut membership = Vec::new();
        let mut content_object_refs = Vec::new();
        let mut anchors = Vec::new();
        let run_id = crate::dcg::observation::ObservationRunId(format!(
            "run_{}",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        ));
        for entry in observed.resources {
            if let Some(digest) = &entry.state.content_digest {
                content_object_refs.push(digest.clone());
            }
            membership.push(ResourceCoverageMembership {
                resource_id: entry.state.resource_id.clone(),
                domain: entry.coverage_domain.clone(),
            });
            // Capture the anchor from *this* observed generation, under the same
            // fencing that produced the state. A capture taken later would be
            // evidence about a different moment, and Draft would have no way to
            // tell. Where fencing fails, no anchor is written — a missing anchor
            // is honest; a mismatched one is not.
            if let Some(anchor) = crate::dcg::source::ResourceSource::capture_anchor(
                &self.source,
                &entry,
                &crate::dcg::source::AnchorRequest {
                    observation_run_id: run_id.clone(),
                },
            )? {
                anchors.push(anchor);
            }
            resources.push(entry.state);
        }

        let snapshot = Snapshot {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::WorkspaceSnapshot,
            ),
            id: SnapshotId::generate(),
            workspace_id: self.workspace.workspace_id.clone(),
            observation_context_digest: observation_context_digest.to_string(),
            resources,
            observation_map: SnapshotObservationMap {
                domains: observed.coverage,
                resource_membership: membership,
            },
            gaps: observed.gaps,
            untrackable: observed.untrackable,
            identity_proofs: Vec::new(),
            content_object_refs,
            created_at: now(),
            created_by: actor,
            snapshot_digest: String::new(),
        }
        .seal();
        snapshot.validate()?;
        write_json(
            &self.workspace.layout.snapshot_file(snapshot.id.as_str()),
            &snapshot,
        )?;
        let provenance_digest = self.record_provenance(&snapshot, run_id, started_at)?;
        let anchor_set = crate::dcg::anchor::RecoveryAnchorSet::build(&snapshot, anchors)?;
        write_json(
            &self
                .workspace
                .layout
                .recovery_anchor_file(&snapshot.snapshot_digest),
            &anchor_set,
        )?;
        Ok((snapshot, provenance_digest))
    }

    /// Record which implementation actually performed this observation.
    ///
    /// Deliberately not an input to any state digest: a semantics-preserving
    /// upgrade must not change what the state *is*, while history must still be
    /// able to say which build observed it. Draft's own filesystem observer is
    /// Core, not an extension, so its provenance carries a component and a
    /// revision — never a fabricated producer, attestation or grant.
    ///
    /// The store is an append-only multimap keyed by `snapshot_digest`. The
    /// same state observed again later is a *different* historical observation
    /// and gets its own record; re-recording an identical assembly is
    /// idempotent because the file is named by the assembly's own digest.
    fn record_provenance(
        &self,
        snapshot: &Snapshot,
        run_id: crate::dcg::observation::ObservationRunId,
        started_at: crate::support::common::Timestamp,
    ) -> DraftResult<String> {
        use crate::dcg::observation as obs;
        let domains: Vec<CoverageDomainRef> = snapshot
            .observation_map
            .domains
            .iter()
            .map(|coverage| coverage.domain.clone())
            .collect();
        let completed_at = now();
        let outcome = if snapshot.gaps.is_empty() {
            obs::ObservationRunOutcome::Succeeded
        } else {
            obs::ObservationRunOutcome::Partial {
                gap_ids: snapshot.gaps.iter().map(|gap| gap.gap_id.clone()).collect(),
            }
        };
        let provenance = obs::ObservationRunProvenance::build(
            snapshot.snapshot_digest.clone(),
            Some(snapshot.id.clone()),
            vec![obs::ObservationRun {
                run_id,
                binding_id: filesystem_binding_id(),
                effective_binding_digest: snapshot.observation_context_digest.clone(),
                observation_provider: obs::ObservationProvider::Core {
                    component: FILESYSTEM_BINDING.to_string(),
                    implementation_revision: FILESYSTEM_OBSERVER_REVISION,
                },
                // One walk over one universe: what it attempted is what it
                // committed, and it owns every domain in the snapshot exactly
                // once.
                attempted_domains: domains.clone(),
                committed_domains: domains,
                authorization_decisions: Vec::new(),
                executable_identity: None,
                environment_metadata_digest: None,
                operation_ids: Vec::new(),
                started_at,
                completed_at,
                outcome,
            }],
            Vec::new(),
            completed_at,
        );
        provenance.validate(&snapshot.observation_map)?;
        write_json(
            &self.workspace.layout.observation_provenance_file(
                &snapshot.snapshot_digest,
                &provenance.provenance_digest,
            ),
            &provenance,
        )?;
        Ok(provenance.provenance_digest)
    }
}

pub(crate) fn read_ignore_lines(path: &Path) -> DraftResult<Vec<String>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(fs::read_to_string(path)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToString::to_string)
        .collect())
}

pub(crate) fn pattern_match(pattern: &str, path: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    if pattern == ".draft/" {
        return pathguard::is_draft_path(path);
    }
    if let Some(directory) = pattern.strip_suffix("/**") {
        return path == directory || path.starts_with(&format!("{directory}/"));
    }
    if let Some(directory) = pattern.strip_suffix('/') {
        return path == directory || path.starts_with(&format!("{directory}/"));
    }
    if pattern.contains('*') {
        return crate::support::glob::matches(&pattern, path)
            || path
                .rsplit('/')
                .next()
                .is_some_and(|name| crate::support::glob::matches(&pattern, name));
    }
    path == pattern || path.starts_with(&format!("{pattern}/"))
}

pub(crate) fn relative_path(root: &Path, path: &Path) -> DraftResult<WorkspacePath> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| DraftError::storage("path escaped workspace root"))?;
    Ok(WorkspacePath::from_relative(relative))
}

pub(crate) fn walk_dir<F: FnMut(&Path) -> DraftResult<()>>(
    root: &Path,
    callback: &mut F,
) -> DraftResult<()> {
    if !root.exists() {
        return Ok(());
    }
    if root.is_file() || root.is_symlink() {
        return callback(root);
    }
    let entries = fs::read_dir(root).map_err(|error| {
        DraftError::storage(format!("cannot read directory {}: {error}", root.display()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            DraftError::storage(format!(
                "cannot read entry under {}: {error}",
                root.display()
            ))
        })?;
        let path = entry.path();
        if path.file_name().is_some_and(|name| name == ".draft") {
            continue;
        }
        if path.is_dir() {
            walk_dir(&path, callback)?;
        } else {
            callback(&path)?;
        }
    }
    Ok(())
}

fn executable(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

pub(crate) fn latest_snapshot(workspace: &Workspace) -> DraftResult<Option<Snapshot>> {
    let directory = workspace.layout.snapshots_dir();
    if !directory.exists() {
        return Ok(None);
    }
    let mut newest: Option<(std::time::SystemTime, Snapshot)> = None;
    for entry in fs::read_dir(&directory)?.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let snapshot: Snapshot = crate::contracts::read_persisted(&path)?;
        let modified = entry.metadata()?.modified()?;
        if newest
            .as_ref()
            .is_none_or(|(previous, _)| modified > *previous)
        {
            newest = Some((modified, snapshot));
        }
    }
    Ok(newest.map(|(_, snapshot)| snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_patterns_can_only_add_to_what_is_skipped() {
        let matcher = IgnoreMatcher {
            patterns: vec!["build/".into(), "*.tmp".into()],
        };
        assert!(matcher.is_ignored("build/out.bin"));
        assert!(matcher.is_ignored("notes.tmp"));
        assert!(matcher.is_ignored("deep/notes.tmp"));
        assert!(!matcher.is_ignored("src/main.rs"));
        // The control plane is excluded structurally; no ignore file is needed
        // to protect it and none could expose it.
        assert!(pathguard::is_draft_path(".draft/config.toml"));
    }

    #[test]
    fn a_contributed_view_rule_removes_a_resource_from_the_observed_universe() {
        use draft_extension_contract::{RawResourcePredicate, ResourceRule};

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join(".draft")).unwrap();
        fs::write(root.join(".git/config"), b"control").unwrap();
        fs::write(root.join("src.txt"), b"authored").unwrap();
        let workspace = Workspace {
            workspace_id: draft_dcg_contract::ids::ProjectId::parse("prj_view-rules").unwrap(),
            root: root.to_path_buf(),
            layout: crate::project::layout::DraftLayout::for_root(root),
        };

        let bodies = |rules: Vec<ResourceRule>| -> Vec<String> {
            Scanner::new(&workspace, rules)
                .unwrap()
                .enumerate()
                .unwrap()
                .resources
                .into_iter()
                .map(|observed| observed.state.locator.body)
                // Walk order is not part of the contract; snapshot identity
                // sorts canonically.
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect()
        };

        // With nothing installed, another tool's control directory is ordinary
        // project state: Draft does not know what Git is.
        assert_eq!(bodies(Vec::new()), vec![".git/config", "src.txt"]);

        // The rule `draft.software.project` contributes takes it out of the
        // observed universe entirely — not ignored, not a gap, not observed.
        let excluded = bodies(vec![ResourceRule {
            predicate: RawResourcePredicate::PathGlob {
                glob: ".git/**".into(),
            },
            reason: "an external history tool's control directory".into(),
        }]);
        assert_eq!(excluded, vec!["src.txt"]);

        // And no view rule can reach Draft's own control plane, which is
        // excluded structurally before any rule is consulted.
        let permissive = bodies(vec![ResourceRule {
            predicate: RawResourcePredicate::PathGlob { glob: "zzz".into() },
            reason: "matches nothing".into(),
        }]);
        assert!(!permissive.iter().any(|body| body.starts_with(".draft")));
    }

    #[test]
    fn a_fencing_token_changes_with_the_content_it_fences() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("a.txt");
        fs::write(&path, b"one").unwrap();
        let first = fencing_token(&fs::symlink_metadata(&path).unwrap());
        // A different size is a different generation under any platform's rules.
        fs::write(&path, b"one-and-more").unwrap();
        let second = fencing_token(&fs::symlink_metadata(&path).unwrap());
        assert_ne!(first, second);
    }

    #[test]
    fn an_unreadable_subtree_becomes_a_gap_rather_than_silence() {
        // The failure this design exists to prevent: fewer entries returned, and
        // the next comparison reading the absence as deletions.
        let error =
            DraftError::storage("cannot read directory /x: Permission denied (os error 13)");
        assert_eq!(gap_kind_for(&error), ObservationGapKind::PermissionDenied);
        assert_eq!(stable_code_for(&error), "filesystem.permission_denied");

        let other = DraftError::storage("cannot read directory /x: too many open files");
        assert_eq!(gap_kind_for(&other), ObservationGapKind::EnumerationFailed);
    }

    #[test]
    fn coverage_domains_are_this_adapters_own_partition() {
        let root = domain(ROOT_DOMAIN);
        let sub = domain("src");
        assert_eq!(root.adapter_binding_id, filesystem_binding_id());
        assert_ne!(root, sub);
        // Another adapter's identically named domain is a different domain.
        let foreign = CoverageDomainRef::new(AdapterBindingId("example.catalog".into()), "root");
        assert_ne!(root, foreign);
    }
}
