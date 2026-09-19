//! `InstallationReceipt` — `<install_root>/.draft-install/receipt.json`.
//!
//! The only authority for what this installation owns. Ownership is a closed
//! set of *typed* entries with variant-specific identity — two binaries by
//! `{sha256, size}`, Unix PATH symlinks by their exact target — and the single
//! modelled Windows integration, `windows_path`. There is no generic
//! `owned_paths` list and no `integrations` collection, so a corrupted receipt
//! can never name an arbitrary file for deletion. `.draft-install/` itself is
//! owned *structurally* (it is the authority root), never by a receipt entry.
//!
//! Every read is validated under an explicit context (I74): a final managed
//! receipt everywhere except inside an active matching `FreshInstall`, where the
//! binaries-stage intermediate shape is also legal.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::layout::{Executable, InstallLayout};
use super::path::windows;
use super::{
    fail, Identity, InstallPlatform, InstallationFailure, InstallationId, SUPPORTED_TARGETS,
};
use crate::support::common::Timestamp;
use crate::support::error::{DraftError, DraftResult};

pub const RECEIPT_SCHEMA_VERSION: u32 = 1;

/// The update track an installation follows: exactly two, closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseChannel {
    Stable,
    Prerelease,
}

impl ReleaseChannel {
    pub fn parse(value: &str) -> DraftResult<Self> {
        match value {
            "stable" => Ok(Self::Stable),
            "prerelease" => Ok(Self::Prerelease),
            other => Err(fail(
                InstallationFailure::UnsupportedReleaseChannel,
                format!("'{other}' is not a release channel; use stable or prerelease"),
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Prerelease => "prerelease",
        }
    }
}

/// How the installation was established. Only the official installer produces
/// a receipt, so this is the single legal value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallationMethod {
    OfficialStandalone,
}

/// `bin/draft[.exe]` and its identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftBinaryEntry {
    pub relative_path: String,
    pub sha256: String,
    pub size: u64,
}

/// `bin/draftd[.exe]` and its identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftDaemonBinaryEntry {
    pub relative_path: String,
    pub sha256: String,
    pub size: u64,
}

impl DraftBinaryEntry {
    pub fn identity(&self) -> Identity {
        Identity {
            sha256: self.sha256.clone(),
            size: self.size,
        }
    }
}

impl DraftDaemonBinaryEntry {
    pub fn identity(&self) -> Identity {
        Identity {
            sha256: self.sha256.clone(),
            size: self.size,
        }
    }
}

/// One Unix PATH symlink the installer created: `<path_bin>/draft` →
/// `<install_root>/bin/draft`. Validated by `lstat` and its exact target,
/// never by hashing through it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathSymlinkEntry {
    pub link_path: String,
    pub expected_target: String,
}

/// The observed pre-install state of `HKCU\Environment\Path`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowsUserPathValueOrigin {
    Absent,
    PresentRegSz,
    PresentRegExpandSz,
}

/// The one-time pre-install PATH classification. Immutable for the
/// installation lifetime; only `AddedByDraft` (a durable reservation, not
/// writer attribution) ever grants removal authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowsPathProvenance {
    NotManaged,
    PreExisting,
    AddedByDraft,
}

/// The single modelled Windows integration: `<install_root>\bin` on the User
/// PATH, and whether Draft reserved it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsPathEntry {
    pub segment: String,
    pub provenance: WindowsPathProvenance,
    pub value_pre_install: WindowsUserPathValueOrigin,
}

/// The typed ownership model an uninstall plan is built from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OwnedInstallationEntry {
    DraftBinary {
        relative_path: String,
        sha256: String,
        size: u64,
    },
    DraftDaemonBinary {
        relative_path: String,
        sha256: String,
        size: u64,
    },
    PathSymlink {
        link_path: String,
        expected_target: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallationReceipt {
    pub schema_version: u32,
    pub installation_id: InstallationId,
    pub installed_version: String,
    pub install_generation: u64,
    pub release_channel: ReleaseChannel,
    pub installation_method: InstallationMethod,
    pub platform_target: String,
    pub install_root: String,
    pub draft_executable: DraftBinaryEntry,
    pub draftd_executable: DraftDaemonBinaryEntry,
    pub path_links: Vec<PathSymlinkEntry>,
    /// Always serialized — `null` on Unix, the object on Windows.
    pub windows_path: Option<WindowsPathEntry>,
    pub installed_at: Timestamp,
}

/// What an intermediate (binaries-stage) receipt must agree with: the active
/// `FreshInstall` journal.
#[derive(Debug, Clone)]
pub struct IntermediateExpectation {
    pub installation_id: InstallationId,
    pub target_version: String,
    pub target_release_channel: ReleaseChannel,
    pub target_draft_identity: Identity,
    pub target_draftd_identity: Identity,
}

/// The two legal validation contexts (I74).
#[derive(Debug, Clone)]
pub enum ReceiptContext {
    /// Every read outside an active matching `FreshInstall`.
    FinalManaged,
    /// Only inside an active matching `FreshInstall`, at the phases I74 lists.
    FreshInstallIntermediate(IntermediateExpectation),
}

fn invalid(message: impl Into<String>) -> DraftError {
    fail(InstallationFailure::InstallationReceiptInvalid, message)
}

impl InstallationReceipt {
    pub fn owned_entries(&self) -> Vec<OwnedInstallationEntry> {
        let mut entries = vec![
            OwnedInstallationEntry::DraftBinary {
                relative_path: self.draft_executable.relative_path.clone(),
                sha256: self.draft_executable.sha256.clone(),
                size: self.draft_executable.size,
            },
            OwnedInstallationEntry::DraftDaemonBinary {
                relative_path: self.draftd_executable.relative_path.clone(),
                sha256: self.draftd_executable.sha256.clone(),
                size: self.draftd_executable.size,
            },
        ];
        entries.extend(
            self.path_links
                .iter()
                .map(|link| OwnedInstallationEntry::PathSymlink {
                    link_path: link.link_path.clone(),
                    expected_target: link.expected_target.clone(),
                }),
        );
        entries
    }

    /// Whether this is the binaries-stage shape: no path ownership recorded yet.
    pub fn is_intermediate(&self) -> bool {
        self.path_links.is_empty() && self.windows_path.is_none()
    }

    /// Validate every field against its frozen slot under `context`.
    pub fn validate(&self, layout: &InstallLayout, context: &ReceiptContext) -> DraftResult<()> {
        if self.schema_version != RECEIPT_SCHEMA_VERSION {
            return Err(invalid(format!(
                "receipt schema {} is not supported",
                self.schema_version
            )));
        }
        if !InstallationId::is_well_formed(self.installation_id.as_str()) {
            return Err(invalid("receipt installation_id is malformed"));
        }
        if semver::Version::parse(&self.installed_version).is_err() {
            return Err(invalid("receipt installed_version is not a SemVer version"));
        }
        if self.install_generation == 0 {
            return Err(invalid("receipt install_generation starts at 1"));
        }
        if !SUPPORTED_TARGETS.contains(&self.platform_target.as_str()) {
            return Err(invalid(format!(
                "receipt names unsupported target {}",
                self.platform_target
            )));
        }
        if Path::new(&self.install_root) != layout.root() {
            return Err(invalid(format!(
                "receipt install_root {} is not this installation's root {}",
                self.install_root,
                layout.root().display()
            )));
        }
        if self.draft_executable.relative_path != layout.relative(Executable::Draft)
            || !self.draft_executable.identity().is_well_formed()
        {
            return Err(invalid(
                "receipt draft_executable is outside its slot or malformed",
            ));
        }
        if self.draftd_executable.relative_path != layout.relative(Executable::Draftd)
            || !self.draftd_executable.identity().is_well_formed()
        {
            return Err(invalid(
                "receipt draftd_executable is outside its slot or malformed",
            ));
        }
        match context {
            ReceiptContext::FinalManaged => self.validate_final_matrix(layout),
            ReceiptContext::FreshInstallIntermediate(expected) => {
                if !self.is_intermediate() {
                    return self.validate_final_matrix(layout);
                }
                let matches = self.installation_id == expected.installation_id
                    && self.installed_version == expected.target_version
                    && self.install_generation == 1
                    && self.release_channel == expected.target_release_channel
                    && self.draft_executable.identity() == expected.target_draft_identity
                    && self.draftd_executable.identity() == expected.target_draftd_identity;
                if matches {
                    Ok(())
                } else {
                    Err(invalid(
                        "an intermediate receipt does not match the active FreshInstall journal",
                    ))
                }
            }
        }
    }

    /// The final platform matrix: Unix has exactly two managed links and a
    /// `null` `windows_path`; Windows has no links and a decided `windows_path`.
    fn validate_final_matrix(&self, layout: &InstallLayout) -> DraftResult<()> {
        match layout.platform() {
            InstallPlatform::Unix => {
                if self.windows_path.is_some() {
                    return Err(invalid("a Unix receipt carries no windows_path"));
                }
                if self.path_links.len() != 2 {
                    return Err(invalid(
                        "a final Unix receipt records exactly the two managed PATH symlinks",
                    ));
                }
                let mut seen = Vec::new();
                for link in &self.path_links {
                    let path = PathBuf::from(&link.link_path);
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let executable = match name.as_str() {
                        "draft" => Executable::Draft,
                        "draftd" => Executable::Draftd,
                        _ => return Err(invalid("a PATH symlink entry names no Draft slot")),
                    };
                    if !path.is_absolute()
                        || Path::new(&link.expected_target) != layout.executable(executable)
                        || seen.contains(&name)
                    {
                        return Err(invalid(
                            "a PATH symlink entry targets something other than its binary slot",
                        ));
                    }
                    seen.push(name);
                }
                let parents: Vec<_> = self
                    .path_links
                    .iter()
                    .map(|link| {
                        PathBuf::from(&link.link_path)
                            .parent()
                            .map(Path::to_path_buf)
                    })
                    .collect();
                if parents[0] != parents[1] {
                    return Err(invalid(
                        "the two PATH symlinks must share one PATH directory",
                    ));
                }
                Ok(())
            }
            InstallPlatform::Windows => {
                if !self.path_links.is_empty() {
                    return Err(invalid("a Windows receipt records no PATH symlinks"));
                }
                let Some(entry) = &self.windows_path else {
                    return Err(invalid(
                        "a final Windows receipt must record its windows_path decision",
                    ));
                };
                let expected = windows::canonical_segment(layout);
                if !windows::is_owned_match(&entry.segment, &expected) {
                    return Err(invalid(
                        "windows_path.segment is not this installation's <install_root>\\bin",
                    ));
                }
                Ok(())
            }
        }
    }

    /// Parse, requiring `windows_path` to be present (never omitted).
    pub fn from_json(bytes: &[u8]) -> DraftResult<Self> {
        let value: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|error| invalid(format!("receipt is not JSON: {error}")))?;
        if value.get("windows_path").is_none() {
            return Err(invalid("receipt omits windows_path"));
        }
        serde_json::from_value(value)
            .map_err(|error| invalid(format!("receipt is malformed: {error}")))
    }

    pub fn to_json(&self) -> DraftResult<Vec<u8>> {
        serde_json::to_vec_pretty(self)
            .map_err(|error| DraftError::storage(format!("serialize receipt: {error}")))
    }
}

/// Read the receipt if present. Malformed is `InstallationReceiptInvalid`.
pub fn read(layout: &InstallLayout) -> DraftResult<Option<InstallationReceipt>> {
    match std::fs::read(layout.receipt()) {
        Ok(bytes) => InstallationReceipt::from_json(&bytes).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(DraftError::storage(format!("read receipt: {error}"))),
    }
}

/// Read and validate a final managed receipt; absent is an error.
pub fn read_final(layout: &InstallLayout) -> DraftResult<InstallationReceipt> {
    let receipt = read(layout)?.ok_or_else(|| invalid("this installation has no receipt"))?;
    receipt.validate(layout, &ReceiptContext::FinalManaged)?;
    Ok(receipt)
}

/// Atomically (re)write the receipt: temp → sync → rename → directory sync.
pub fn write(layout: &InstallLayout, receipt: &InstallationReceipt) -> DraftResult<()> {
    super::write_private_bytes(&layout.receipt(), &receipt.to_json()?)
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub fn identity(seed: &str) -> Identity {
        Identity {
            sha256: crate::support::hashing::sha256_hex(seed.as_bytes())
                .trim_start_matches("sha256:")
                .to_string(),
            size: seed.len() as u64,
        }
    }

    pub fn unix(layout: &InstallLayout, path_bin: &Path) -> InstallationReceipt {
        InstallationReceipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            installation_id: InstallationId::new("ins_0123456789ab"),
            installed_version: "0.3.4".into(),
            install_generation: 1,
            release_channel: ReleaseChannel::Stable,
            installation_method: InstallationMethod::OfficialStandalone,
            platform_target: "x86_64-unknown-linux-musl".into(),
            install_root: layout.root().display().to_string(),
            draft_executable: DraftBinaryEntry {
                relative_path: layout.relative(Executable::Draft),
                sha256: identity("draft").sha256,
                size: 5,
            },
            draftd_executable: DraftDaemonBinaryEntry {
                relative_path: layout.relative(Executable::Draftd),
                sha256: identity("draftd").sha256,
                size: 6,
            },
            path_links: vec![
                PathSymlinkEntry {
                    link_path: path_bin.join("draft").display().to_string(),
                    expected_target: layout.executable(Executable::Draft).display().to_string(),
                },
                PathSymlinkEntry {
                    link_path: path_bin.join("draftd").display().to_string(),
                    expected_target: layout.executable(Executable::Draftd).display().to_string(),
                },
            ],
            windows_path: None,
            installed_at: chrono::DateTime::from_timestamp(1_780_000_000, 0).unwrap(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    fn windows_layout() -> InstallLayout {
        InstallLayout::new(
            r"C:\Users\ada\AppData\Local\Programs\Draft",
            InstallPlatform::Windows,
        )
    }

    fn windows_receipt(provenance: WindowsPathProvenance) -> InstallationReceipt {
        let layout = windows_layout();
        let mut receipt = unix(&layout, Path::new("/unused"));
        receipt.platform_target = "x86_64-pc-windows-msvc".into();
        receipt.path_links.clear();
        receipt.windows_path = Some(WindowsPathEntry {
            segment: windows::canonical_segment(&layout),
            provenance,
            value_pre_install: WindowsUserPathValueOrigin::Absent,
        });
        receipt
    }

    #[test]
    fn a_unix_receipt_round_trips_with_windows_path_null() {
        let layout = InstallLayout::new("/opt/root", InstallPlatform::Unix);
        let receipt = unix(&layout, Path::new("/opt/pathbin"));
        receipt
            .validate(&layout, &ReceiptContext::FinalManaged)
            .unwrap();
        let json = String::from_utf8(receipt.to_json().unwrap()).unwrap();
        assert!(json.contains("\"windows_path\": null"), "{json}");
        assert_eq!(
            InstallationReceipt::from_json(json.as_bytes()).unwrap(),
            receipt
        );
        assert_eq!(receipt.owned_entries().len(), 4);
    }

    #[test]
    fn a_windows_receipt_records_all_three_path_fields_for_each_provenance() {
        let layout = windows_layout();
        for (provenance, wire) in [
            (WindowsPathProvenance::NotManaged, "not_managed"),
            (WindowsPathProvenance::PreExisting, "pre_existing"),
            (WindowsPathProvenance::AddedByDraft, "added_by_draft"),
        ] {
            let receipt = windows_receipt(provenance);
            receipt
                .validate(&layout, &ReceiptContext::FinalManaged)
                .unwrap();
            let value = serde_json::to_value(&receipt).unwrap();
            assert_eq!(value["path_links"], serde_json::json!([]));
            assert_eq!(value["windows_path"]["provenance"], wire);
            assert_eq!(value["windows_path"]["value_pre_install"], "absent");
            assert!(value["windows_path"]["segment"]
                .as_str()
                .unwrap()
                .ends_with(r"\bin"));
        }
    }

    #[test]
    fn the_platform_matrix_fails_closed() {
        let unix_layout = InstallLayout::new("/opt/root", InstallPlatform::Unix);
        let mut receipt = unix(&unix_layout, Path::new("/opt/pathbin"));
        receipt.path_links.pop();
        assert!(receipt
            .validate(&unix_layout, &ReceiptContext::FinalManaged)
            .is_err());

        let layout = windows_layout();
        let mut windows = windows_receipt(WindowsPathProvenance::AddedByDraft);
        windows.windows_path = None;
        assert!(windows
            .validate(&layout, &ReceiptContext::FinalManaged)
            .is_err());
        let mut windows = windows_receipt(WindowsPathProvenance::AddedByDraft);
        windows.path_links = unix(&unix_layout, Path::new("/p")).path_links;
        assert!(windows
            .validate(&layout, &ReceiptContext::FinalManaged)
            .is_err());
    }

    #[test]
    fn a_receipt_cannot_widen_ownership_to_a_sibling() {
        let layout = InstallLayout::new("/opt/root", InstallPlatform::Unix);
        let mut receipt = unix(&layout, Path::new("/opt/pathbin"));
        receipt.draft_executable.relative_path = "../other/draft".into();
        assert!(receipt
            .validate(&layout, &ReceiptContext::FinalManaged)
            .is_err());
        let mut receipt = unix(&layout, Path::new("/opt/pathbin"));
        receipt.path_links[0].expected_target = "/usr/bin/python3".into();
        assert!(receipt
            .validate(&layout, &ReceiptContext::FinalManaged)
            .is_err());
        let mut receipt = unix(&layout, Path::new("/opt/pathbin"));
        receipt.path_links[0].link_path = "/opt/pathbin/unrelated".into();
        assert!(receipt
            .validate(&layout, &ReceiptContext::FinalManaged)
            .is_err());
    }

    #[test]
    fn unknown_fields_values_and_a_missing_windows_path_fail_closed() {
        let layout = windows_layout();
        let good =
            serde_json::to_value(windows_receipt(WindowsPathProvenance::NotManaged)).unwrap();
        let mut unknown = good.clone();
        unknown["windows_path"]["provenance"] = serde_json::json!("adopted");
        assert!(InstallationReceipt::from_json(unknown.to_string().as_bytes()).is_err());
        let mut unknown = good.clone();
        unknown["windows_path"]["value_pre_install"] = serde_json::json!("present_reg_binary");
        assert!(InstallationReceipt::from_json(unknown.to_string().as_bytes()).is_err());
        let mut extra = good.clone();
        extra["windows_path"]["whole_path"] = serde_json::json!("C:\\x");
        assert!(InstallationReceipt::from_json(extra.to_string().as_bytes()).is_err());
        let mut missing = good.clone();
        missing["windows_path"]
            .as_object_mut()
            .unwrap()
            .remove("value_pre_install");
        assert!(InstallationReceipt::from_json(missing.to_string().as_bytes()).is_err());
        let mut omitted = good.clone();
        omitted.as_object_mut().unwrap().remove("windows_path");
        assert!(InstallationReceipt::from_json(omitted.to_string().as_bytes()).is_err());
        let mut owned = good;
        owned["owned_paths"] = serde_json::json!(["C:\\Windows"]);
        assert!(InstallationReceipt::from_json(owned.to_string().as_bytes()).is_err());
        let _ = layout;
    }

    #[test]
    fn an_intermediate_receipt_is_legal_only_against_its_fresh_install() {
        let layout = InstallLayout::new("/opt/root", InstallPlatform::Unix);
        let mut receipt = unix(&layout, Path::new("/opt/pathbin"));
        receipt.path_links.clear();
        assert!(receipt
            .validate(&layout, &ReceiptContext::FinalManaged)
            .is_err());
        let expected = IntermediateExpectation {
            installation_id: receipt.installation_id.clone(),
            target_version: "0.3.4".into(),
            target_release_channel: ReleaseChannel::Stable,
            target_draft_identity: receipt.draft_executable.identity(),
            target_draftd_identity: receipt.draftd_executable.identity(),
        };
        receipt
            .validate(
                &layout,
                &ReceiptContext::FreshInstallIntermediate(expected.clone()),
            )
            .unwrap();
        let mut wrong = expected.clone();
        wrong.installation_id = InstallationId::new("ins_ffffffffffff");
        assert!(receipt
            .validate(&layout, &ReceiptContext::FreshInstallIntermediate(wrong))
            .is_err());
        let mut wrong = expected;
        wrong.target_draft_identity = identity("other");
        assert!(receipt
            .validate(&layout, &ReceiptContext::FreshInstallIntermediate(wrong))
            .is_err());
    }
}
