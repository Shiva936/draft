//! Exporting an accepted Baseline, and importing one into quarantine.
//!
//! A DraftPack carries an accepted Baseline out of one Draft installation so
//! another can verify it — without trusting the sender. The signed manifest
//! from `draft-draftpack-contract` is what makes that possible: it lists every
//! member with its exact digest, so a recipient checks the bytes it actually
//! received against what the exporter said it was sending.
//!
//! # Export and import are deliberately asymmetric
//!
//! Export reads state Draft already accepted. Import reads bytes from
//! anywhere, so it validates the archive completely — path safety, then the
//! signed manifest, then per-member digests — before a byte reaches the
//! quarantine.
//!
//! # Import grants nothing
//!
//! A pack that verifies is a well-formed, authentically signed set of claims.
//! It is not an accepted Baseline here. Whether the signing key is trusted in
//! *this* project is the importer's question, and answering it from inside the
//! archive would let a sender vouch for itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use draft_draftpack_contract::manifest::{
    ArchiveEntryMetadata, DraftpackManifest, MANIFEST_ENTRY_PATH,
};
use draft_draftpack_contract::path::SafeEntryPath;
use draft_draftpack_contract::{DraftpackEnvelope, DRAFTPACK_FORMAT_REVISION};

use draft_dcg_contract::ids::ActorId;
use draft_dcg_contract::receipt::ReceiptSignerBinding;
use draft_dcg_contract::value::Timestamp;
use draft_dcg_contract::{BaselineId, ProducerIdentity};

use crate::draftpack::{read_archive, write_archive};
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil;
use crate::trust::signing::Keypair;

/// What an export produced.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExportReport {
    pub baseline: String,
    pub artifact: PathBuf,
    /// The manifest's own canonical digest, which the signature covers.
    pub manifest_digest: String,
    pub members: usize,
}

/// What an import established, and what it deliberately did not.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImportReport {
    pub baseline: String,
    pub manifest_digest: String,
    pub members: usize,
    /// Whether the exporter's signature covers exactly this manifest.
    ///
    /// Level 2 of three. It says the bytes are unchanged and that whoever held
    /// the key produced them — never that the key is trusted here.
    pub signature_verified: bool,
    /// Always false. Stated rather than implied: a verified pack is a set of
    /// claims until this project accepts them through its own Promotion, and a
    /// field that said so only by omission would be read as an oversight.
    pub locally_accepted: bool,
    /// Receipts the pack carried, retained as history and granting nothing.
    pub external_receipts: Vec<String>,
    /// Where the quarantined members landed. Absent for a dry run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarantine: Option<PathBuf>,
}

fn invalid(message: impl Into<String>) -> DraftError {
    DraftError::new(DraftErrorKind::Validation, message)
        .with_suggestion("an imported DraftPack must match its signed manifest exactly")
}

fn format_error(error: draft_draftpack_contract::FormatError) -> DraftError {
    DraftError::new(DraftErrorKind::Validation, error.to_string())
}

/// The members of one Baseline export, in canonical path order.
pub type Members = BTreeMap<SafeEntryPath, Vec<u8>>;

/// Assemble the members describing one accepted Baseline.
pub fn members_for_baseline(
    layout: &crate::project::layout::DraftLayout,
    baseline: &BaselineId,
) -> DraftResult<Members> {
    let store = crate::dcg::baseline::BaselineStore::new(layout.baselines_dir());
    let manifest = store.manifest(baseline)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::NotFound,
            format!("this project holds no Baseline {baseline}"),
        )
    })?;
    let record = store.record(baseline)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::CorruptData,
            format!("Baseline {baseline} has a manifest but no acceptance record"),
        )
    })?;
    // The composition is exported because the evidence root is a digest: it
    // proves the entries did not change and cannot say what they were. Without
    // it a recipient could verify the Baseline and still not know which
    // provider established any part of it.
    let composition = store.composition(baseline)?.ok_or_else(|| {
        DraftError::new(
            DraftErrorKind::CorruptData,
            format!("Baseline {baseline} has no recorded composition"),
        )
    })?;
    let lineage = store.lineage(baseline)?;

    let mut members = Members::new();
    for (path, bytes) in [
        ("baseline/manifest.json", encode(&manifest)?),
        ("baseline/record.json", encode(&record)?),
        ("baseline/composition.json", encode(&composition)?),
        (
            "baseline/lineage.json",
            encode(&lineage.iter().map(ToString::to_string).collect::<Vec<_>>())?,
        ),
    ] {
        members.insert(SafeEntryPath::parse(path).map_err(format_error)?, bytes);
    }
    Ok(members)
}

/// Write one accepted Baseline to `out` as a signed DraftPack.
#[allow(clippy::too_many_arguments)]
pub fn export(
    baseline: &BaselineId,
    project: draft_dcg_contract::ids::ProjectId,
    members: Members,
    receipts: Vec<draft_dcg_contract::receipt::ReceiptEnvelope>,
    exported_by: ActorId,
    exported_at: Timestamp,
    producer: ProducerIdentity,
    signer: ReceiptSignerBinding,
    keypair: &Keypair,
    out: &Path,
) -> DraftResult<ExportReport> {
    let mut entries = std::collections::BTreeSet::new();
    for (path, bytes) in &members {
        entries.insert(ArchiveEntryMetadata::describe(path.clone(), bytes).map_err(format_error)?);
    }

    let manifest = DraftpackManifest {
        format_revision: DRAFTPACK_FORMAT_REVISION,
        project,
        baseline: baseline.clone(),
        entries,
        receipts,
        exported_by,
        exported_at,
        producer,
    };
    let manifest_digest = manifest.digest().map_err(format_error)?;

    let message = draft_draftpack_contract::envelope::DraftpackSigningMessage {
        manifest: manifest.clone(),
        signer: signer.clone(),
    };
    let envelope = DraftpackEnvelope {
        manifest,
        signer,
        signature: keypair.sign_b64(&message.signing_bytes().map_err(format_error)?),
    };

    let mut archive: Vec<(String, Vec<u8>)> = members
        .into_iter()
        .map(|(path, bytes)| (path.as_str().to_string(), bytes))
        .collect();
    archive.push((MANIFEST_ENTRY_PATH.to_string(), encode(&envelope)?));

    if let Some(parent) = out.parent() {
        fsutil::ensure_dir(parent)?;
    }
    write_archive(out, &archive)?;

    Ok(ExportReport {
        baseline: baseline.to_string(),
        artifact: out.to_path_buf(),
        manifest_digest: manifest_digest.to_string(),
        members: archive.len(),
    })
}

/// Validate an untrusted `.draftpack`, and — unless this is a dry run — place
/// its members in quarantine.
pub fn import(
    layout: &crate::project::layout::DraftLayout,
    artifact: &Path,
    dry_run: bool,
) -> DraftResult<ImportReport> {
    let archive = read_archive(artifact)?;

    let envelope_bytes = archive
        .entries
        .get(MANIFEST_ENTRY_PATH)
        .ok_or_else(|| invalid("the archive carries no draftpack.json"))?;
    let envelope: DraftpackEnvelope = serde_json::from_slice(envelope_bytes)
        .map_err(|error| invalid(format!("draftpack.json does not parse: {error}")))?;
    envelope.verify_structure().map_err(format_error)?;

    // Every member, checked against what the exporter said it was sending. A
    // single archive digest would prove only that the bytes are unchanged in
    // transit; per-entry digests say *which* member is wrong when one is, and
    // make a silently added or removed member a manifest mismatch rather than
    // an unnoticed difference.
    let mut contents = Members::new();
    for (name, bytes) in &archive.entries {
        if name == MANIFEST_ENTRY_PATH {
            continue;
        }
        contents.insert(
            SafeEntryPath::parse(name.clone()).map_err(format_error)?,
            bytes.clone(),
        );
    }
    let report = envelope.manifest.verify(&contents);
    if !report.is_complete() {
        return Err(invalid(format!(
            "the archive does not match its signed manifest: {report}"
        )));
    }

    // Level 2 of three. Whether the key is trusted here is level 3, and it is
    // the importer's question — answering it from inside the archive would let
    // a sender vouch for itself.
    let home = crate::project::home::DraftGlobalStore::locate()?;
    let signature_verified =
        crate::trust::identity::global::resolve_public_key(&home, &envelope.signer.signing_key_id)
            .ok()
            .flatten()
            .and_then(|key| envelope.verify_signature_with(&key).ok())
            .unwrap_or(false);

    let baseline = envelope.manifest.baseline.to_string();
    let manifest_digest = envelope
        .manifest
        .digest()
        .map_err(format_error)?
        .to_string();
    let external_receipts = envelope
        .manifest
        .receipts
        .iter()
        .map(|receipt| receipt.payload.receipt_id.to_string())
        .collect();

    if dry_run {
        return Ok(ImportReport {
            baseline,
            manifest_digest,
            members: archive.entries.len(),
            signature_verified,
            locally_accepted: false,
            external_receipts,
            quarantine: None,
        });
    }

    // One directory per imported Baseline. A second pack claiming the same
    // Baseline is refused rather than merged: two archives claiming one
    // identity are two different claims, and overwriting one with the other
    // would destroy the evidence that they disagreed.
    let directory = layout.quarantine_dir().join(baseline.replace(':', "-"));
    if directory.exists() {
        return Err(invalid(format!(
            "Baseline {baseline} is already in quarantine; remove it before importing another \
             pack claiming it"
        )));
    }
    fsutil::ensure_dir(&directory)?;
    for (name, bytes) in &archive.entries {
        let path = directory.join(name);
        if let Some(parent) = path.parent() {
            fsutil::ensure_dir(parent)?;
        }
        fsutil::write_atomic(&path, bytes)?;
    }

    Ok(ImportReport {
        baseline,
        manifest_digest,
        members: archive.entries.len(),
        signature_verified,
        locally_accepted: false,
        external_receipts,
        quarantine: Some(directory),
    })
}

fn encode<T: serde::Serialize>(value: &T) -> DraftResult<Vec<u8>> {
    serde_json::to_vec_pretty(value)
        .map(|mut bytes| {
            bytes.push(b'\n');
            bytes
        })
        .map_err(|error| DraftError::storage(format!("cannot encode archive member: {error}")))
}
