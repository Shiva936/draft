//! Active changepack graph and composition validation.

use crate::error::{DraftError, DraftErrorKind, DraftResult};
use crate::hashing;
use crate::pack::{PackLockfile, PackManifest};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relationship {
    Independent,
    Dependent,
    Conflicting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompositionStatus {
    Created,
    Verified,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Composition {
    pub id: String,
    pub base_stable_head: String,
    pub pack_ids: Vec<String>,
    pub composition_hash: String,
    pub dependency_order: Vec<String>,
    pub status: CompositionStatus,
    pub affected_paths: Vec<String>,
    pub conflicts: Vec<String>,
}

pub fn classify(a: &PackLockfile, b: &PackLockfile) -> Relationship {
    let a_files: BTreeSet<_> = a.file_hashes.keys().cloned().collect();
    let b_files: BTreeSet<_> = b.file_hashes.keys().cloned().collect();
    if a_files.intersection(&b_files).next().is_some() {
        return Relationship::Conflicting;
    }
    if a.dependency_pack_hashes.iter().any(|d| d == &b.pack_id)
        || b.dependency_pack_hashes.iter().any(|d| d == &a.pack_id)
    {
        return Relationship::Dependent;
    }
    Relationship::Independent
}

/// Validate a composition of packs against a base stable head.
///
/// `external_conflicts` lets a hunk-aware caller (the compare/compose
/// pipeline, which permits same-file non-overlapping hunks per TDD §11.4)
/// supply the authoritative conflict set; `None` falls back to the coarse
/// file-overlap classification from [`classify`].
pub fn validate(
    base_stable_head: &str,
    manifests: &[PackManifest],
    locks: &[PackLockfile],
    external_conflicts: Option<Vec<String>>,
) -> DraftResult<Composition> {
    let mut by_id: BTreeMap<String, &PackLockfile> = BTreeMap::new();
    for lock in locks {
        by_id.insert(lock.pack_id.clone(), lock);
    }
    let mut affected_paths = BTreeSet::new();
    let mut derived_conflicts = Vec::new();
    for i in 0..locks.len() {
        for j in (i + 1)..locks.len() {
            if classify(&locks[i], &locks[j]) == Relationship::Conflicting {
                derived_conflicts.push(format!(
                    "{} conflicts with {}",
                    locks[i].pack_id, locks[j].pack_id
                ));
            }
        }
        affected_paths.extend(locks[i].file_hashes.keys().cloned());
    }
    let conflicts = external_conflicts.unwrap_or(derived_conflicts);
    if !conflicts.is_empty() {
        return Ok(build(
            base_stable_head,
            manifests,
            locks,
            Vec::new(),
            affected_paths,
            conflicts,
            CompositionStatus::Failed,
        ));
    }
    let dependency_order = topo_sort(locks, &by_id)?;
    Ok(build(
        base_stable_head,
        manifests,
        locks,
        dependency_order,
        affected_paths,
        Vec::new(),
        CompositionStatus::Verified,
    ))
}

fn build(
    base_stable_head: &str,
    manifests: &[PackManifest],
    locks: &[PackLockfile],
    dependency_order: Vec<String>,
    affected_paths: BTreeSet<String>,
    conflicts: Vec<String>,
    status: CompositionStatus,
) -> Composition {
    let pack_ids = manifests
        .iter()
        .map(|m| m.pack_id.clone())
        .collect::<Vec<_>>();
    let mut c = Composition {
        id: format!("cmp_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
        base_stable_head: base_stable_head.to_string(),
        pack_ids,
        composition_hash: String::new(),
        dependency_order,
        status,
        affected_paths: affected_paths.into_iter().collect(),
        conflicts,
    };
    c.composition_hash = hashing::canonical_hash(&serde_json::json!({
        "base_stable_head": c.base_stable_head,
        "pack_ids": c.pack_ids,
        "dependency_order": c.dependency_order,
        "affected_paths": c.affected_paths,
        "conflicts": c.conflicts,
        "locks": locks,
    }));
    c
}

fn topo_sort(
    locks: &[PackLockfile],
    by_id: &BTreeMap<String, &PackLockfile>,
) -> DraftResult<Vec<String>> {
    let mut incoming: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for lock in locks {
        let deps = lock
            .dependency_pack_hashes
            .iter()
            .filter(|dep| by_id.contains_key(*dep))
            .cloned()
            .collect::<BTreeSet<_>>();
        incoming.insert(lock.pack_id.clone(), deps);
    }
    let mut out = Vec::new();
    loop {
        let ready = incoming
            .iter()
            .find(|(_, deps)| deps.is_empty())
            .map(|(id, _)| id.clone());
        let Some(id) = ready else {
            break;
        };
        incoming.remove(&id);
        for deps in incoming.values_mut() {
            deps.remove(&id);
        }
        out.push(id);
    }
    if !incoming.is_empty() {
        return Err(DraftError::new(
            DraftErrorKind::ConflictDetected,
            "composition dependency cycle detected",
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::LockedCommand;

    fn lock(id: &str, files: &[&str], deps: &[&str]) -> PackLockfile {
        PackLockfile {
            schema_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            pack_id: id.to_string(),
            workspace_hash: hashing::sha256_hex(id.as_bytes()),
            file_hashes: files
                .iter()
                .map(|f| (f.to_string(), hashing::sha256_hex(f.as_bytes())))
                .collect(),
            policy_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            risk_engine_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            verification_commands: Vec::<LockedCommand>::new(),
            lsif_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            test_selector_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            fuzz_selector_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            dependency_pack_hashes: deps.iter().map(|d| d.to_string()).collect(),
            receipt_hashes: Vec::new(),
        }
    }

    #[test]
    fn classifies_path_overlap_as_conflict() {
        assert_eq!(
            classify(
                &lock("pck_a", &["src/lib.rs"], &[]),
                &lock("pck_b", &["src/lib.rs"], &[])
            ),
            Relationship::Conflicting
        );
    }

    #[test]
    fn topological_order_respects_dependencies() {
        let locks = vec![
            lock("pck_b", &["b"], &["pck_a"]),
            lock("pck_a", &["a"], &[]),
        ];
        let by_id = locks
            .iter()
            .map(|lock| (lock.pack_id.clone(), lock))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(topo_sort(&locks, &by_id).unwrap(), vec!["pck_a", "pck_b"]);
    }

    fn manifest(id: &str) -> PackManifest {
        PackManifest {
            schema_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
            pack_id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            intent: crate::pack::PackIntent::Feature,
            origin: "local".to_string(),
            actor: "test".to_string(),
            candidate: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            base_workspace_hash: String::new(),
            target_workspace_hash: String::new(),
            changes_hash: String::new(),
            risk_hash: String::new(),
            verify_hash: String::new(),
            lsif_hash: String::new(),
            receipt_hashes: Vec::new(),
            import_state: crate::pack::ImportState::None,
            approval_state: crate::pack::ApprovalState::Pending,
            save_state: crate::pack::SaveState::Unsaved,
        }
    }

    #[test]
    fn validate_verifies_independent_and_dependent_packs_in_order() {
        let manifests = vec![manifest("pck_a"), manifest("pck_b")];
        let locks = vec![
            lock("pck_b", &["b"], &["pck_a"]),
            lock("pck_a", &["a"], &[]),
        ];
        let c = validate("rcp_base", &manifests, &locks, None).unwrap();
        assert_eq!(c.status, CompositionStatus::Verified);
        assert_eq!(c.dependency_order, vec!["pck_a", "pck_b"]);
        assert!(c.conflicts.is_empty());
        assert!(!c.composition_hash.is_empty());
        // Deterministic hash across runs.
        let c2 = validate("rcp_base", &manifests, &locks, None).unwrap();
        assert_eq!(c.composition_hash, c2.composition_hash);
    }

    #[test]
    fn validate_derives_file_overlap_conflicts_without_external_input() {
        let manifests = vec![manifest("pck_a"), manifest("pck_b")];
        let locks = vec![
            lock("pck_a", &["src/lib.rs"], &[]),
            lock("pck_b", &["src/lib.rs"], &[]),
        ];
        let c = validate("rcp_base", &manifests, &locks, None).unwrap();
        assert_eq!(c.status, CompositionStatus::Failed);
        assert_eq!(c.conflicts.len(), 1);
    }

    #[test]
    fn validate_prefers_external_hunk_aware_conflicts() {
        let manifests = vec![manifest("pck_a"), manifest("pck_b")];
        let locks = vec![
            lock("pck_a", &["src/lib.rs"], &[]),
            lock("pck_b", &["src/lib.rs"], &[]),
        ];
        // A hunk-aware caller found no real overlap: composition verifies.
        let c = validate("rcp_base", &manifests, &locks, Some(Vec::new())).unwrap();
        assert_eq!(c.status, CompositionStatus::Verified);
        // A hunk-aware caller found a real conflict: composition fails.
        let c = validate(
            "rcp_base",
            &manifests,
            &locks,
            Some(vec!["src/lib.rs: overlapping hunks".to_string()]),
        )
        .unwrap();
        assert_eq!(c.status, CompositionStatus::Failed);
    }

    #[test]
    fn validate_fails_on_dependency_cycles() {
        let manifests = vec![manifest("pck_a"), manifest("pck_b")];
        let locks = vec![
            lock("pck_a", &["a"], &["pck_b"]),
            lock("pck_b", &["b"], &["pck_a"]),
        ];
        assert!(validate("rcp_base", &manifests, &locks, Some(Vec::new())).is_err());
    }
}
