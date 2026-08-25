//! Signed receipts (PRD §9.8, TDD §15).
//!
//! Every trust-relevant action produces an Ed25519-signed receipt stored at
//! `.draft/receipts/rcp_<id>.json`. A receipt binds an event (`event_hash`,
//! `previous_event_hash`) and the workspace state (`workspace_hash`) to the
//! acting identity, and is signed over its canonical content (all fields except
//! `signature`). Verification re-derives the signable bytes and checks the
//! signature, the actor's public key, revocation, and chain linkage.

use crate::support::common::ReceiptId;
use crate::support::error::{DraftError, DraftResult};
use crate::support::fsutil;
use crate::support::hashing;
use crate::trust::event::EventRecord;
use crate::trust::signing::{self, Keypair};
use crate::workspace::layout::DraftLayout;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// In-memory input used to mint an action receipt and its linked event.
#[derive(Debug, Clone)]
pub(crate) struct ActionReceiptDraft {
    pub(crate) id: ReceiptId,
    pub(crate) kind: String,
    pub(crate) status: String,
    pub(crate) subject_id: Option<String>,
    pub(crate) rollback_target: Option<String>,
    pub(crate) payload: serde_json::Value,
}

impl ActionReceiptDraft {
    pub(crate) fn new(
        kind: &str,
        status: &str,
        subject_id: Option<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            id: ReceiptId::generate(),
            kind: kind.to_string(),
            status: status.to_string(),
            subject_id,
            rollback_target: None,
            payload,
        }
    }

    pub(crate) fn reversible_to(mut self, target: impl Into<String>) -> Self {
        self.rollback_target = Some(target.into());
        self
    }
}

/// A signed receipt record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReceiptRecord {
    pub schema_version: u32,
    pub receipt_id: String,
    pub event_type: String,
    pub subject_id: Option<String>,
    pub actor_id: String,
    pub candidate_id: Option<String>,
    pub workspace_id: String,
    pub workspace_hash: String,
    pub event_hash: String,
    pub previous_event_hash: String,
    pub timestamp: String,
    pub policy_version: String,
    pub draft_version: String,
    pub signature_algorithm: String,
    pub public_key_id: String,
    pub signature: String,
}

impl crate::contracts::VersionedContract for ReceiptRecord {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::Receipt;
}

impl ReceiptRecord {
    /// The canonical, signature-excluded bytes that are signed and verified.
    pub fn signable_bytes(&self) -> Vec<u8> {
        let content = json!({
            "schema_version": self.schema_version,
            "receipt_id": self.receipt_id,
            "event_type": self.event_type,
            "subject_id": self.subject_id,
            "actor_id": self.actor_id,
            "candidate_id": self.candidate_id,
            "workspace_id": self.workspace_id,
            "workspace_hash": self.workspace_hash,
            "event_hash": self.event_hash,
            "previous_event_hash": self.previous_event_hash,
            "timestamp": self.timestamp,
            "policy_version": self.policy_version,
            "draft_version": self.draft_version,
            "signature_algorithm": self.signature_algorithm,
            "public_key_id": self.public_key_id,
        });
        hashing::canonical_json(&content).into_bytes()
    }
}

/// Fields required to mint a receipt for an event.
pub struct ReceiptDraft<'a> {
    pub event: &'a EventRecord,
    pub receipt_id: String,
    pub workspace_hash: String,
    pub policy_version: String,
    pub public_key_id: String,
}

/// Mint a signed receipt for `draft.event`, signing with `keypair`.
pub fn create_signed(draft: ReceiptDraft<'_>, keypair: &Keypair) -> ReceiptRecord {
    let mut rec = ReceiptRecord {
        schema_version: crate::contracts::current_version(crate::contracts::ContractId::Receipt),
        receipt_id: draft.receipt_id,
        event_type: draft.event.event_type.clone(),
        subject_id: draft.event.subject_id.clone(),
        actor_id: draft.event.actor_id.clone(),
        candidate_id: draft.event.candidate_id.clone(),
        workspace_id: draft
            .event
            .ledger
            .workspace_id()
            .expect("signed project receipt requires workspace ledger")
            .to_string(),
        workspace_hash: draft.workspace_hash,
        event_hash: draft.event.event_hash.clone(),
        previous_event_hash: draft.event.previous_event_hash.clone(),
        timestamp: crate::support::common::now().to_rfc3339(),
        policy_version: draft.policy_version,
        draft_version: crate::DRAFT_VERSION.to_string(),
        signature_algorithm: signing::SIGNATURE_ALGORITHM.to_string(),
        public_key_id: draft.public_key_id,
        signature: String::new(),
    };
    rec.signature = keypair.sign_b64(&rec.signable_bytes());
    rec
}

/// The outcome of verifying one receipt: a list of named checks and overall ok.
#[derive(Debug, Clone, Serialize)]
pub struct ReceiptVerification {
    pub receipt_id: String,
    pub ok: bool,
    pub checks: Vec<ReceiptCheck>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReceiptCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

fn chk(name: &str, ok: bool, detail: impl Into<String>) -> ReceiptCheck {
    ReceiptCheck {
        name: name.to_string(),
        ok,
        detail: detail.into(),
    }
}

/// Verify a receipt against the event chain, the resolved public key, and the
/// revoked-key set. `public_key` is the base64 key resolved for
/// `receipt.public_key_id` (None if it could not be resolved).
pub fn verify(
    receipt: &ReceiptRecord,
    events: &[EventRecord],
    public_key: Option<&str>,
    revoked: &[String],
) -> ReceiptVerification {
    let mut checks = Vec::new();

    // Schema / algorithm.
    checks.push(chk(
        "schema-version",
        crate::contracts::supports_version(
            crate::contracts::ContractId::Receipt,
            receipt.schema_version,
        ),
        format!("schema_version = {}", receipt.schema_version),
    ));
    checks.push(chk(
        "draft-version-provenance",
        !receipt.draft_version.trim().is_empty(),
        format!("draft_version = {}", receipt.draft_version),
    ));
    checks.push(chk(
        "algorithm",
        receipt.signature_algorithm == signing::SIGNATURE_ALGORITHM,
        format!("algorithm = {}", receipt.signature_algorithm),
    ));

    // Actor public key resolves.
    let key_ok = public_key.is_some();
    checks.push(chk(
        "actor-public-key",
        key_ok,
        if key_ok {
            "resolved".to_string()
        } else {
            format!("cannot resolve {}", receipt.public_key_id)
        },
    ));

    // Not revoked.
    let revoked_ok = !revoked.iter().any(|r| r == &receipt.public_key_id);
    checks.push(chk(
        "revoked-key",
        revoked_ok,
        if revoked_ok {
            "key not revoked"
        } else {
            "KEY REVOKED"
        },
    ));

    // Signature validity.
    let sig_ok = match public_key {
        Some(pk) => {
            signing::verify_b64(pk, &receipt.signable_bytes(), &receipt.signature).unwrap_or(false)
        }
        None => false,
    };
    checks.push(chk(
        "signature",
        sig_ok,
        if sig_ok { "valid" } else { "INVALID" },
    ));

    // Event linkage: an event with this event_hash exists and links match.
    let matched = events.iter().find(|e| e.event_hash == receipt.event_hash);
    let event_ok = matched.is_some();
    checks.push(chk(
        "event-hash",
        event_ok,
        if event_ok {
            "event found"
        } else {
            "no matching event"
        },
    ));
    let prev_ok = matched
        .map(|e| e.previous_event_hash == receipt.previous_event_hash)
        .unwrap_or(false);
    checks.push(chk(
        "previous-event-hash",
        prev_ok,
        if prev_ok {
            "linked"
        } else {
            "previous hash mismatch"
        },
    ));
    let content_ok = matched
        .map(|event| {
            event.event_type == receipt.event_type
                && event.subject_id == receipt.subject_id
                && event.actor_id == receipt.actor_id
                && event.candidate_id == receipt.candidate_id
                && event.receipt_id.as_deref() == Some(receipt.receipt_id.as_str())
        })
        .unwrap_or(false);
    checks.push(chk(
        "event-content",
        content_ok,
        if content_ok {
            "event identity and content match"
        } else {
            "event identity or content mismatch"
        },
    ));
    let ledger_ok = matched
        .and_then(|event| event.ledger.workspace_id())
        .map(|workspace_id| workspace_id == receipt.workspace_id)
        .unwrap_or(false);
    checks.push(chk(
        "workspace-ledger",
        ledger_ok,
        if ledger_ok {
            "receipt is bound to the event workspace ledger"
        } else {
            "receipt workspace does not match the event ledger"
        },
    ));

    // Workspace hash recorded.
    let ws_ok = receipt.workspace_hash.starts_with("sha256:");
    checks.push(chk(
        "workspace-hash",
        ws_ok,
        if ws_ok { "present" } else { "missing/invalid" },
    ));

    let ok = checks.iter().all(|c| c.ok);
    ReceiptVerification {
        receipt_id: receipt.receipt_id.clone(),
        ok,
        checks,
    }
}

/// Persistent receipt store bound to a project.
pub struct ReceiptStore {
    paths: DraftLayout,
}

impl ReceiptStore {
    pub fn new(paths: DraftLayout) -> Self {
        ReceiptStore { paths }
    }

    pub fn write(&self, receipt: &ReceiptRecord) -> DraftResult<()> {
        fsutil::ensure_dir(&self.paths.receipts_dir())?;
        let path = self.paths.receipt_file(&receipt.receipt_id);
        if path.exists() {
            let existing: ReceiptRecord = crate::contracts::read_persisted(&path)?;
            if existing != *receipt {
                return Err(DraftError::new(
                    crate::support::error::DraftErrorKind::ConflictDetected,
                    "immutable signed receipt already exists with different content",
                ));
            }
            return Ok(());
        }
        fsutil::write_json(&path, receipt)
    }

    pub fn read(&self, receipt_id: &str) -> DraftResult<ReceiptRecord> {
        let path = self.paths.receipt_file(receipt_id);
        if !path.exists() {
            return Err(DraftError::not_found(format!(
                "receipt {receipt_id} not found"
            )));
        }
        crate::contracts::read_persisted::<ReceiptRecord>(&path)
    }

    pub fn list(&self) -> DraftResult<Vec<ReceiptRecord>> {
        let mut out = Vec::new();
        for path in fsutil::list_with_extension(&self.paths.receipts_dir(), "json")? {
            out.push(crate::contracts::read_persisted::<ReceiptRecord>(&path)?);
        }
        out.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trust::event::{EventKind, EventLog, NewEvent};

    fn mk_event(root: &std::path::Path) -> EventRecord {
        let log = EventLog::workspace(DraftLayout::for_root(root), "ws_x");
        log.append(NewEvent {
            kind: EventKind::PackCreated,
            subject_id: Some("pck_a".into()),
            actor_id: "act_x".into(),
            candidate_id: None,
            receipt_id: Some("rcp_test".into()),
            metadata: serde_json::json!({}),
        })
        .unwrap()
    }

    #[test]
    fn sign_and_verify_valid_receipt() {
        let tmp = tempfile::tempdir().unwrap();
        let event = mk_event(tmp.path());
        let kp = Keypair::generate();
        let receipt = create_signed(
            ReceiptDraft {
                event: &event,
                receipt_id: "rcp_test".into(),
                workspace_hash: "sha256:abc".into(),
                policy_version: "0.3.3".into(),
                public_key_id: kp.public_key_id(),
            },
            &kp,
        );
        let events = EventLog::workspace(DraftLayout::for_root(tmp.path()), "ws_x")
            .read_all()
            .unwrap();
        let v = verify(&receipt, &events, Some(&kp.public_key_b64()), &[]);
        assert!(v.ok, "checks: {:?}", v.checks);
    }

    #[test]
    fn tampered_receipt_fails_signature() {
        let tmp = tempfile::tempdir().unwrap();
        let event = mk_event(tmp.path());
        let kp = Keypair::generate();
        let mut receipt = create_signed(
            ReceiptDraft {
                event: &event,
                receipt_id: "rcp_test".into(),
                workspace_hash: "sha256:abc".into(),
                policy_version: "0.3.3".into(),
                public_key_id: kp.public_key_id(),
            },
            &kp,
        );
        receipt.subject_id = Some("pck_evil".into()); // tamper after signing
        let events = vec![event];
        let v = verify(&receipt, &events, Some(&kp.public_key_b64()), &[]);
        assert!(!v.ok);
        assert!(v.checks.iter().any(|c| c.name == "signature" && !c.ok));
    }

    #[test]
    fn revoked_key_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let event = mk_event(tmp.path());
        let kp = Keypair::generate();
        let receipt = create_signed(
            ReceiptDraft {
                event: &event,
                receipt_id: "rcp_test".into(),
                workspace_hash: "sha256:abc".into(),
                policy_version: "0.3.3".into(),
                public_key_id: kp.public_key_id(),
            },
            &kp,
        );
        let events = vec![event];
        let v = verify(
            &receipt,
            &events,
            Some(&kp.public_key_b64()),
            &[kp.public_key_id()],
        );
        assert!(!v.ok);
        assert!(v.checks.iter().any(|c| c.name == "revoked-key" && !c.ok));
    }
}
