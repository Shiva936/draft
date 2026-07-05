//! Global identity, candidate registry, and signing-key lifecycle (TDD §11).
//!
//! The global store owns a single **actor** (the human/device operating Draft)
//! and a registry of **candidates** (the humans/AIs/tools/services that produce
//! changes). The actor's Ed25519 private key lives only under `~/.draft/keys`;
//! its public half and a stable `public_key_id` are what receipts reference.

use crate::error::{DraftError, DraftResult};
use crate::fsutil::{read_json, write_json};
use crate::home::GlobalHome;
use crate::signing::Keypair;
use serde::{Deserialize, Serialize};

/// The active actor profile stored at `~/.draft/identity/actor.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActorProfile {
    pub actor_id: String,
    pub display_name: String,
    pub public_key_id: String,
    pub created_at: String,
}

/// A candidate (change producer) in `~/.draft/identity/candidates.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CandidateRecord {
    pub candidate_id: String,
    pub kind: CandidateKind,
    pub name: String,
    pub provider: String,
    pub created_at: String,
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
    pub public_key_id: String,
    pub public_key: String,
    pub algorithm: String,
    pub actor_id: String,
}

/// Reported by `draft identity status`.
#[derive(Debug, Clone, Serialize)]
pub struct IdentityStatus {
    pub actor: Option<ActorProfile>,
    pub signing_key_available: bool,
    pub candidate_count: usize,
}

/// Ensure the global actor + signing key exist, creating them on first use.
/// Idempotent: returns the existing profile if already provisioned.
pub fn ensure_actor(home: &GlobalHome) -> DraftResult<ActorProfile> {
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
                actor_id: crate::common::ActorId::generate().to_string(),
                display_name: default_display_name(),
                public_key_id: public_key_id.clone(),
                created_at: crate::common::now().to_rfc3339(),
            };
            write_json(&home.actor_json(), &profile)?;
            profile
        }
    };
    publish_public_key(home, &profile, &keypair)?;
    Ok(profile)
}

/// Load the actor profile if it exists.
pub fn load_actor(home: &GlobalHome) -> DraftResult<Option<ActorProfile>> {
    if !home.actor_json().exists() {
        return Ok(None);
    }
    Ok(Some(read_json::<ActorProfile>(&home.actor_json())?))
}

/// Load the signing keypair for the active actor.
pub fn load_keypair(home: &GlobalHome) -> DraftResult<Keypair> {
    if !home.signing_key().exists() {
        return Err(DraftError::not_found(
            "no signing key; run `draft init --global`",
        ));
    }
    Keypair::load(&home.signing_key())
}

/// Load the signing key and reconcile the active actor/public key metadata to
/// the exact key that will be used for new signatures.
pub fn active_signer(home: &GlobalHome) -> DraftResult<(ActorProfile, Keypair)> {
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
    home: &GlobalHome,
    profile: &ActorProfile,
    keypair: &Keypair,
) -> DraftResult<()> {
    let public_key_id = keypair.public_key_id();
    let pub_record = PublicKeyRecord {
        public_key_id: public_key_id.clone(),
        public_key: keypair.public_key_b64(),
        algorithm: crate::signing::SIGNATURE_ALGORITHM.to_string(),
        actor_id: profile.actor_id.clone(),
    };
    write_json(
        &home.public_keys_dir().join(format!("{public_key_id}.json")),
        &pub_record,
    )
}

/// Resolve a `public_key_id` to its base64 public key from the published keys.
pub fn resolve_public_key(home: &GlobalHome, public_key_id: &str) -> DraftResult<Option<String>> {
    let path = home.public_keys_dir().join(format!("{public_key_id}.json"));
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(read_json::<PublicKeyRecord>(&path)?.public_key))
}

/// Report identity status for `draft identity status`.
pub fn status(home: &GlobalHome) -> DraftResult<IdentityStatus> {
    let actor = load_actor(home)?;
    Ok(IdentityStatus {
        actor,
        signing_key_available: home.signing_key().exists(),
        candidate_count: list_candidates(home)?.len(),
    })
}

/// Load the candidate registry (empty if none).
pub fn list_candidates(home: &GlobalHome) -> DraftResult<Vec<CandidateRecord>> {
    if !home.candidates_json().exists() {
        return Ok(Vec::new());
    }
    read_json::<Vec<CandidateRecord>>(&home.candidates_json())
}

/// Register (or return existing) a candidate by name+kind. Idempotent on name.
pub fn register_candidate(
    home: &GlobalHome,
    name: &str,
    kind: CandidateKind,
    provider: &str,
) -> DraftResult<CandidateRecord> {
    let mut all = list_candidates(home)?;
    if let Some(existing) = all.iter().find(|c| c.name == name) {
        return Ok(existing.clone());
    }
    let rec = CandidateRecord {
        candidate_id: format!("cnd_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
        kind,
        name: name.to_string(),
        provider: provider.to_string(),
        created_at: crate::common::now().to_rfc3339(),
    };
    all.push(rec.clone());
    home.create_all()?;
    write_json(&home.candidates_json(), &all)?;
    Ok(rec)
}

fn default_display_name() -> String {
    for var in ["USER", "USERNAME", "LOGNAME"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                return v;
            }
        }
    }
    "Local User".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_actor_is_idempotent_and_creates_key() {
        let tmp = tempfile::tempdir().unwrap();
        let home = GlobalHome::at(tmp.path().join(".draft"));
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
        let home = GlobalHome::at(tmp.path().join(".draft"));
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
        let home = GlobalHome::at(tmp.path().join(".draft"));
        let c1 = register_candidate(&home, "local-agent", CandidateKind::Ai, "local").unwrap();
        let c2 = register_candidate(&home, "local-agent", CandidateKind::Ai, "local").unwrap();
        assert_eq!(c1.candidate_id, c2.candidate_id);
        assert_eq!(list_candidates(&home).unwrap().len(), 1);
    }

    #[test]
    fn status_reflects_provisioning() {
        let tmp = tempfile::tempdir().unwrap();
        let home = GlobalHome::at(tmp.path().join(".draft"));
        let s = status(&home).unwrap();
        assert!(s.actor.is_none() && !s.signing_key_available);
        ensure_actor(&home).unwrap();
        let s = status(&home).unwrap();
        assert!(s.actor.is_some() && s.signing_key_available);
    }
}
