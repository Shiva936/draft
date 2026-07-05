//! Trust ledger — the atomic façade over the event log, signed receipts, and
//! the transparency chain (TDD §13–16).
//!
//! Every trust-relevant action goes through [`TrustLedger::record`], which
//! appends a canonical hash-chained event, mints an Ed25519-signed receipt
//! binding that event to the workspace hash, links the receipt id back onto the
//! event, and appends the receipt into the transparency chain.
//!
//! Verification ([`TrustLedger::verify_all`]) re-checks all three structures and
//! every receipt signature, resolving public keys from the global store and
//! honoring the revoked-key list. All failures are reported, never swallowed.

use crate::error::DraftResult;
use crate::event::{EventKind, EventLog, EventRecord, NewEvent};
use crate::home::GlobalHome;
use crate::identity::global::{self, ActorProfile};
use crate::layout::ProjectPaths;
use crate::receipt::{self, ReceiptRecord, ReceiptStore, ReceiptVerification};
use crate::signing::Keypair;
use crate::transparency::TransparencyLog;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// The artifacts produced by recording one trust action.
#[derive(Debug, Clone)]
pub struct RecordOutcome {
    pub event: EventRecord,
    pub receipt: ReceiptRecord,
}

/// Aggregate verification result for `draft receipt verify --all` / `doctor`.
#[derive(Debug, Clone, Serialize)]
pub struct LedgerVerification {
    pub event_chain_ok: bool,
    pub event_count: usize,
    pub transparency_ok: bool,
    pub transparency_count: usize,
    pub receipts: Vec<ReceiptVerification>,
    pub all_ok: bool,
}

/// A trust ledger bound to a project workspace and the global signing identity.
pub struct TrustLedger {
    paths: ProjectPaths,
    home: GlobalHome,
    actor: ActorProfile,
    keypair: Keypair,
    workspace_id: String,
}

impl TrustLedger {
    /// Open the ledger for `root`, auto-provisioning the global actor + signing
    /// key on first use (offline, local-first).
    pub fn open(root: &Path, workspace_id: &str) -> DraftResult<Self> {
        Self::open_at(root, workspace_id, GlobalHome::locate()?)
    }

    /// Open the ledger against an explicit global store (used by tests and any
    /// caller that has already resolved the global home).
    pub fn open_at(root: &Path, workspace_id: &str, home: GlobalHome) -> DraftResult<Self> {
        let (actor, keypair) = global::active_signer(&home)?;
        Ok(TrustLedger {
            paths: ProjectPaths::for_root(root),
            home,
            actor,
            keypair,
            workspace_id: workspace_id.to_string(),
        })
    }

    pub fn actor_id(&self) -> &str {
        &self.actor.actor_id
    }

    fn events(&self) -> EventLog {
        EventLog::new(self.paths.clone())
    }
    fn receipts(&self) -> ReceiptStore {
        ReceiptStore::new(self.paths.clone())
    }
    fn transparency(&self) -> TransparencyLog {
        TransparencyLog::new(self.paths.clone())
    }

    /// Record a trust-relevant action end-to-end, returning the event + receipt.
    pub fn record(
        &self,
        kind: EventKind,
        subject_id: Option<String>,
        candidate_id: Option<String>,
        workspace_hash: String,
        metadata: serde_json::Value,
    ) -> DraftResult<RecordOutcome> {
        let events = self.events();
        let receipt_id = format!("rcp_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
        let event = events.append(NewEvent {
            kind,
            subject_id,
            actor_id: self.actor.actor_id.clone(),
            candidate_id,
            workspace_id: self.workspace_id.clone(),
            receipt_id: Some(receipt_id.clone()),
            metadata,
        })?;

        let receipt = receipt::create_signed(
            receipt::ReceiptDraft {
                event: &event,
                receipt_id,
                workspace_hash,
                policy_version: crate::DRAFT_SCHEMA_VERSION.to_string(),
                public_key_id: self.keypair.public_key_id(),
            },
            &self.keypair,
        );
        self.receipts().write(&receipt)?;
        self.transparency().append(
            &receipt.receipt_id,
            &event.event_hash,
            &self.actor.actor_id,
            &receipt.public_key_id,
            &self.keypair,
        )?;

        Ok(RecordOutcome { event, receipt })
    }

    /// Resolve a public key id to its base64 key from the global published keys.
    fn resolve_key(&self, public_key_id: &str) -> Option<String> {
        global::resolve_public_key(&self.home, public_key_id)
            .ok()
            .flatten()
    }

    fn revoked_keys(&self) -> Vec<String> {
        let path = self.home.revoked_keys_json();
        if !path.exists() {
            return Vec::new();
        }
        crate::fsutil::read_json::<Vec<String>>(&path).unwrap_or_default()
    }

    /// Verify a single receipt by id.
    pub fn verify_receipt(&self, receipt_id: &str) -> DraftResult<ReceiptVerification> {
        let receipt = self.receipts().read(receipt_id)?;
        let events = self.events().read_all()?;
        let public_key = self.resolve_key(&receipt.public_key_id);
        Ok(receipt::verify(
            &receipt,
            &events,
            public_key.as_deref(),
            &self.revoked_keys(),
        ))
    }

    /// Verify the full ledger: event chain, transparency chain, every receipt.
    pub fn verify_all(&self) -> DraftResult<LedgerVerification> {
        let events = self.events().read_all()?;
        let event_chain = self.events().verify_chain();
        let (event_chain_ok, event_count) = match event_chain {
            Ok(n) => (true, n),
            Err(_) => (false, events.len()),
        };

        let revoked = self.revoked_keys();
        let mut receipt_results = Vec::new();
        let receipts = self.receipts().list()?;
        let mut receipt_keys = BTreeMap::new();
        for receipt in receipts {
            let public_key = self.resolve_key(&receipt.public_key_id);
            receipt_keys.insert(receipt.receipt_id.clone(), receipt.public_key_id.clone());
            receipt_results.push(receipt::verify(
                &receipt,
                &events,
                public_key.as_deref(),
                &revoked,
            ));
        }

        let transparency = self.transparency();
        let (transparency_ok, transparency_count) = match transparency
            .verify(|entry| self.resolve_transparency_key(entry, &receipt_keys))
        {
            Ok(n) => (true, n),
            Err(_) => (false, transparency.read_all().map(|v| v.len()).unwrap_or(0)),
        };

        let all_ok = event_chain_ok && transparency_ok && receipt_results.iter().all(|r| r.ok);
        Ok(LedgerVerification {
            event_chain_ok,
            event_count,
            transparency_ok,
            transparency_count,
            receipts: receipt_results,
            all_ok,
        })
    }

    fn resolve_transparency_key(
        &self,
        entry: &crate::transparency::TransparencyEntry,
        receipt_keys: &BTreeMap<String, String>,
    ) -> Option<String> {
        if let Some(public_key_id) = &entry.public_key_id {
            return self.resolve_key(public_key_id);
        }
        if let Some(public_key_id) = receipt_keys.get(&entry.receipt_id) {
            return self.resolve_key(public_key_id);
        }
        if entry.actor_id == self.actor.actor_id {
            return self.resolve_key(&self.actor.public_key_id);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(root: &Path) {
        // Isolate a global store per test.
        ProjectPaths::for_root(root).create_all().unwrap();
    }

    #[test]
    fn record_produces_verifiable_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let home = GlobalHome::at(tmp.path().join("global/.draft"));
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        setup(&root);

        let ledger = TrustLedger::open_at(&root, "ws_test", home).unwrap();
        let out = ledger
            .record(
                EventKind::PackCreated,
                Some("pck_a".into()),
                Some("cnd_ai".into()),
                "sha256:deadbeef".into(),
                serde_json::json!({"intent": "feature"}),
            )
            .unwrap();
        assert_eq!(
            out.event.receipt_id.as_deref(),
            Some(out.receipt.receipt_id.as_str())
        );
        assert_eq!(out.receipt.event_hash, out.event.event_hash);

        let v = ledger.verify_all().unwrap();
        assert!(v.all_ok, "ledger not ok: {v:?}");
        assert_eq!(v.event_count, 1);
        assert_eq!(v.receipts.len(), 1);
        assert!(v.transparency_ok);

        // Single-receipt verify path.
        let single = ledger.verify_receipt(&out.receipt.receipt_id).unwrap();
        assert!(single.ok);
    }

    #[test]
    fn tampered_receipt_detected_by_verify_all() {
        let tmp = tempfile::tempdir().unwrap();
        let home = GlobalHome::at(tmp.path().join("g/.draft"));
        let root = tmp.path().join("p");
        std::fs::create_dir_all(&root).unwrap();
        setup(&root);
        let ledger = TrustLedger::open_at(&root, "ws_x", home).unwrap();
        let out = ledger
            .record(
                EventKind::PackSaved,
                Some("pck_z".into()),
                None,
                "sha256:1".into(),
                serde_json::json!({}),
            )
            .unwrap();
        // Corrupt the receipt on disk.
        let rpath = ProjectPaths::for_root(&root).receipt_file(&out.receipt.receipt_id);
        let mut r: ReceiptRecord = crate::fsutil::read_json(&rpath).unwrap();
        r.workspace_hash = "sha256:tampered".into();
        crate::fsutil::write_json(&rpath, &r).unwrap();

        let v = ledger.verify_all().unwrap();
        assert!(!v.all_ok);
    }

    #[test]
    fn replaced_signing_key_records_verifiable_receipts() {
        let tmp = tempfile::tempdir().unwrap();
        let home = GlobalHome::at(tmp.path().join("g/.draft"));
        let root = tmp.path().join("p");
        std::fs::create_dir_all(&root).unwrap();
        setup(&root);

        let first_actor = global::ensure_actor(&home).unwrap();
        let replacement = Keypair::generate();
        replacement.save(&home.signing_key()).unwrap();

        let ledger = TrustLedger::open_at(&root, "ws_x", home.clone()).unwrap();
        let out = ledger
            .record(
                EventKind::PackCreated,
                Some("pck_drift".into()),
                None,
                "sha256:1".into(),
                serde_json::json!({}),
            )
            .unwrap();

        assert_eq!(ledger.actor.actor_id, first_actor.actor_id);
        assert_eq!(out.receipt.public_key_id, replacement.public_key_id());
        let v = ledger.verify_all().unwrap();
        assert!(v.all_ok, "ledger not ok: {v:?}");
    }

    #[test]
    fn key_rotation_keeps_historical_entries_verifiable() {
        let tmp = tempfile::tempdir().unwrap();
        let home = GlobalHome::at(tmp.path().join("g/.draft"));
        let root = tmp.path().join("p");
        std::fs::create_dir_all(&root).unwrap();
        setup(&root);

        let first = TrustLedger::open_at(&root, "ws_x", home.clone()).unwrap();
        let first_out = first
            .record(
                EventKind::PackCreated,
                Some("pck_rotate".into()),
                None,
                "sha256:1".into(),
                serde_json::json!({}),
            )
            .unwrap();
        let first_key = first_out.receipt.public_key_id.clone();

        let replacement = Keypair::generate();
        replacement.save(&home.signing_key()).unwrap();
        let second = TrustLedger::open_at(&root, "ws_x", home).unwrap();
        let second_out = second
            .record(
                EventKind::PackVerified,
                Some("pck_rotate".into()),
                None,
                "sha256:2".into(),
                serde_json::json!({}),
            )
            .unwrap();

        assert_ne!(first_key, second_out.receipt.public_key_id);
        assert_eq!(
            second_out.receipt.public_key_id,
            replacement.public_key_id()
        );
        let v = second.verify_all().unwrap();
        assert!(v.all_ok, "ledger not ok: {v:?}");
        assert_eq!(v.receipts.len(), 2);
        assert_eq!(v.transparency_count, 2);
    }
}
