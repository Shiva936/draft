//! Global identity, candidate registry, and signing-key lifecycle.
//!
//! See `docs/internals/security.md`.
//!
//! The global store owns a single **actor** (the human/device operating Draft)
//! and a registry of **candidates** (the humans/AIs/tools/services that produce
//! changes). The actor's Ed25519 private key lives only under `~/.draft/keys`;
//! its public half and a stable `public_key_id` are what receipts reference.

use crate::project::home::DraftGlobalStore;
use crate::support::error::{DraftError, DraftResult};
use crate::support::fsutil::write_json;
use crate::trust::signing::Keypair;
use serde::{Deserialize, Serialize};

/// Stable security actor state stored at `~/.draft/identity/actor.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActorProfile {
    pub schema_version: u32,
    pub actor_id: String,
    pub public_key_id: String,
    pub created_at: String,
}

impl crate::contracts::VersionedContract for ActorProfile {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::SecurityActor;
}

/// A candidate (change producer) in `~/.draft/identity/candidates.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CandidateRecord {
    pub schema_version: u32,
    pub candidate_id: String,
    pub kind: CandidateKind,
    pub name: String,
    pub provider: String,
    pub created_at: String,
}

impl crate::contracts::VersionedContract for CandidateRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::CandidateRecord;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CandidateKind {
    Human,
    Ai,
    Tool,
    Service,
    Unknown,
}

/// A published public key, stored under `~/.draft/keys/public.keys/<id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicKeyRecord {
    pub schema_version: u32,
    pub public_key_id: String,
    pub public_key: String,
    pub algorithm: String,
    pub actor_id: String,
}

impl crate::contracts::VersionedContract for PublicKeyRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::PublicKeyRecord;
}

/// Read-only security actor/key status used by diagnostics and Console.
#[derive(Debug, Clone, Serialize)]
pub struct IdentityStatus {
    pub actor: Option<ActorProfile>,
    pub signing_key_available: bool,
    pub candidate_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CandidateRegistryEnvelope {
    schema_version: u32,
    candidates: Vec<CandidateRecord>,
}

impl crate::contracts::VersionedContract for CandidateRegistryEnvelope {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::CandidateRegistry;
}

/// Ensure the global actor + signing key exist, creating them on first use.
/// Idempotent: returns the existing profile if already provisioned.
pub fn ensure_actor(home: &DraftGlobalStore) -> DraftResult<ActorProfile> {
    super::local::reject_retired_profile_state(None)?;
    home.create_all()?;

    let existing = load_actor(home)?;
    let keypair = if home.signing_key().exists() {
        Keypair::load(&home.signing_key())?
    } else {
        let keypair = Keypair::generate();
        keypair.save(&home.signing_key())?;
        keypair
    };
    let public_key_id = keypair.public_key_id();

    let profile = match existing {
        Some(mut profile) => {
            if profile.public_key_id != public_key_id {
                profile.public_key_id = public_key_id.clone();
                write_json(&home.actor_json(), &profile)?;
            }
            profile
        }
        None => {
            let profile = ActorProfile {
                schema_version: crate::contracts::current_version(
                    crate::contracts::ContractId::SecurityActor,
                ),
                actor_id: crate::support::common::ActorId::generate().to_string(),
                public_key_id: public_key_id.clone(),
                created_at: crate::support::common::now().to_rfc3339(),
            };
            write_json(&home.actor_json(), &profile)?;
            profile
        }
    };
    publish_public_key(home, &profile, &keypair)?;
    Ok(profile)
}

/// Load the actor profile if it exists.
pub fn load_actor(home: &DraftGlobalStore) -> DraftResult<Option<ActorProfile>> {
    if !home.actor_json().exists() {
        return Ok(None);
    }
    reject_retired_actor_profile(home)?;
    let bytes = std::fs::read(home.actor_json())?;
    Ok(Some(crate::contracts::decode_persisted::<ActorProfile>(
        &bytes,
    )?))
}

/// Detect the retired mutable fields of the former combined actor/profile
/// record without deserializing or applying them.
pub fn reject_retired_actor_profile(home: &DraftGlobalStore) -> DraftResult<()> {
    if !home.actor_json().exists() {
        return Ok(());
    }
    let bytes = std::fs::read(home.actor_json())?;
    if bytes
        .windows(b"\"display_name\"".len())
        .any(|part| part == b"\"display_name\"")
        || bytes
            .windows(b"\"email\"".len())
            .any(|part| part == b"\"email\"")
    {
        return Err(DraftError::new(
            crate::support::error::DraftErrorKind::UnsupportedSchema,
            format!(
                "unsupported pre-release profile fields exist in {}",
                home.actor_json().display()
            ),
        )
        .with_suggestion("remove the retired combined actor/profile state; profile values are not migrated or applied"));
    }
    Ok(())
}

/// Load the signing keypair for the active actor.
pub fn load_keypair(home: &DraftGlobalStore) -> DraftResult<Keypair> {
    if !home.signing_key().exists() {
        return Err(DraftError::not_found(
            "no signing key; run `draft init --global`",
        ));
    }
    Keypair::load(&home.signing_key())
}

/// Load the signing key and reconcile the active actor/public key metadata to
/// the exact key that will be used for new signatures.
pub fn active_signer(home: &DraftGlobalStore) -> DraftResult<(ActorProfile, Keypair)> {
    let mut actor = ensure_actor(home)?;
    let keypair = load_keypair(home)?;
    let public_key_id = keypair.public_key_id();
    if actor.public_key_id != public_key_id {
        actor.public_key_id = public_key_id;
        write_json(&home.actor_json(), &actor)?;
        publish_public_key(home, &actor, &keypair)?;
    }
    Ok((actor, keypair))
}

fn publish_public_key(
    home: &DraftGlobalStore,
    profile: &ActorProfile,
    keypair: &Keypair,
) -> DraftResult<()> {
    let public_key_id = keypair.public_key_id();
    let pub_record = PublicKeyRecord {
        schema_version: crate::contracts::current_version(
            crate::contracts::ContractId::PublicKeyRecord,
        ),
        public_key_id: public_key_id.clone(),
        public_key: keypair.public_key_b64(),
        algorithm: crate::trust::signing::SIGNATURE_ALGORITHM.to_string(),
        actor_id: profile.actor_id.clone(),
    };
    write_json(
        &home.public_keys_dir().join(format!("{public_key_id}.json")),
        &pub_record,
    )
}

/// Resolve a `public_key_id` to its base64 public key from the published keys.
pub fn resolve_public_key(
    home: &DraftGlobalStore,
    public_key_id: &str,
) -> DraftResult<Option<String>> {
    let path = home.public_keys_dir().join(format!("{public_key_id}.json"));
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(
        crate::contracts::read_persisted::<PublicKeyRecord>(&path)?.public_key,
    ))
}

/// Report stable security actor and signing-key status.
pub fn status(home: &DraftGlobalStore) -> DraftResult<IdentityStatus> {
    let actor = load_actor(home)?;
    Ok(IdentityStatus {
        actor,
        signing_key_available: home.signing_key().exists(),
        candidate_count: list_candidates(home)?.len(),
    })
}

/// Load the candidate registry (empty if none).
pub fn list_candidates(home: &DraftGlobalStore) -> DraftResult<Vec<CandidateRecord>> {
    if !home.candidates_json().exists() {
        return Ok(Vec::new());
    }
    Ok(
        crate::contracts::read_persisted::<CandidateRegistryEnvelope>(&home.candidates_json())?
            .candidates,
    )
}

/// Register (or return existing) a candidate by name+kind. Idempotent on name.
pub fn register_candidate(
    home: &DraftGlobalStore,
    name: &str,
    kind: CandidateKind,
    provider: &str,
) -> DraftResult<CandidateRecord> {
    let mut all = list_candidates(home)?;
    if let Some(existing) = all.iter().find(|c| c.name == name) {
        return Ok(existing.clone());
    }
    let rec = CandidateRecord {
        schema_version: crate::contracts::current_version(
            crate::contracts::ContractId::CandidateRecord,
        ),
        candidate_id: format!("cnd_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
        kind,
        name: name.to_string(),
        provider: provider.to_string(),
        created_at: crate::support::common::now().to_rfc3339(),
    };
    all.push(rec.clone());
    home.create_all()?;
    write_json(
        &home.candidates_json(),
        &CandidateRegistryEnvelope {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::CandidateRegistry,
            ),
            candidates: all,
        },
    )?;
    Ok(rec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_actor_is_idempotent_and_creates_key() {
        let tmp = tempfile::tempdir().unwrap();
        let home = DraftGlobalStore::at(tmp.path().join(".draft"));
        let a = ensure_actor(&home).unwrap();
        assert!(a.public_key_id.starts_with("key_ed25519_"));
        assert!(home.signing_key().exists());
        let b = ensure_actor(&home).unwrap();
        assert_eq!(a, b); // stable across calls
                          // Public key resolves back.
        let pk = resolve_public_key(&home, &a.public_key_id).unwrap();
        assert!(pk.is_some());
    }

    #[test]
    fn ensure_actor_reconciles_replaced_signing_key() {
        let tmp = tempfile::tempdir().unwrap();
        let home = DraftGlobalStore::at(tmp.path().join(".draft"));
        let first = ensure_actor(&home).unwrap();
        let old_key = first.public_key_id.clone();

        let replacement = Keypair::generate();
        replacement.save(&home.signing_key()).unwrap();
        let reconciled = ensure_actor(&home).unwrap();

        assert_eq!(reconciled.actor_id, first.actor_id);
        assert_ne!(reconciled.public_key_id, old_key);
        assert_eq!(reconciled.public_key_id, replacement.public_key_id());
        assert!(resolve_public_key(&home, &old_key).unwrap().is_some());
        assert!(resolve_public_key(&home, &replacement.public_key_id())
            .unwrap()
            .is_some());
    }

    #[test]
    fn candidates_register_and_dedupe() {
        let tmp = tempfile::tempdir().unwrap();
        let home = DraftGlobalStore::at(tmp.path().join(".draft"));
        let c1 = register_candidate(&home, "local-agent", CandidateKind::Ai, "local").unwrap();
        let c2 = register_candidate(&home, "local-agent", CandidateKind::Ai, "local").unwrap();
        assert_eq!(c1.candidate_id, c2.candidate_id);
        assert_eq!(list_candidates(&home).unwrap().len(), 1);
    }

    #[test]
    fn status_reflects_provisioning() {
        let tmp = tempfile::tempdir().unwrap();
        let home = DraftGlobalStore::at(tmp.path().join(".draft"));
        let s = status(&home).unwrap();
        assert!(s.actor.is_none() && !s.signing_key_available);
        ensure_actor(&home).unwrap();
        let s = status(&home).unwrap();
        assert!(s.actor.is_some() && s.signing_key_available);
    }

    #[test]
    fn combined_actor_profile_is_rejected_before_typed_decode() {
        let tmp = tempfile::tempdir().unwrap();
        let home = DraftGlobalStore::at(tmp.path().join(".draft"));
        std::fs::create_dir_all(home.identity_dir()).unwrap();
        std::fs::write(
            home.actor_json(),
            br#"{"display_name":"retired","schema_version":[not valid JSON}"#,
        )
        .unwrap();
        let error = load_actor(&home).unwrap_err();
        assert_eq!(
            error.kind,
            crate::support::error::DraftErrorKind::UnsupportedSchema
        );
        assert!(error
            .message
            .contains("unsupported pre-release profile fields"));
    }
}
