//! Who owns this `draft` — proven, never inferred.
//!
//! * `OfficialStandalone`: the executable canonicalizes to
//!   `<install_root>/bin/draft[.exe]`, and a receipt there validates
//!   completely, including the running binary's identity.
//! * `PackageManagerOwned`: only with a manager-specific ownership query naming
//!   this executable. v0.3.4 ships the classification with **zero** adapters,
//!   because no package-manager distribution exists, so it is never reported.
//! * `SourceOrDevelopment`: under a Cargo `target/{debug,release}/`.
//! * `Unknown`: anything else — including a receipt-less legacy copy — refused
//!   with guidance. Writability is not ownership.

use std::path::{Path, PathBuf};

use super::layout::{self, Executable, InstallLayout};
use super::receipt::{self, InstallationReceipt, ReceiptContext};
use super::{fail, Identity, InstallPlatform, InstallationFailure};
use crate::support::error::{DraftError, DraftResult};

#[derive(Debug, Clone)]
pub enum Provenance {
    OfficialStandalone {
        layout: InstallLayout,
        receipt: Box<InstallationReceipt>,
    },
    PackageManagerOwned {
        manager: String,
        command: String,
    },
    SourceOrDevelopment {
        executable: PathBuf,
    },
    Unknown {
        executable: PathBuf,
        reason: String,
    },
}

fn under_cargo_target(path: &Path) -> bool {
    let parts: Vec<String> = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts
        .windows(2)
        .any(|pair| pair[0] == "target" && (pair[1] == "debug" || pair[1] == "release"))
        || parts
            .windows(3)
            .any(|triple| triple[0] == "target" && (triple[2] == "debug" || triple[2] == "release"))
}

/// Classify the running (or named) `draft` executable.
pub fn detect(executable: &Path, platform: InstallPlatform) -> Provenance {
    let canonical = layout::canonicalize(executable).unwrap_or_else(|_| executable.to_path_buf());
    if under_cargo_target(&canonical) {
        return Provenance::SourceOrDevelopment {
            executable: canonical,
        };
    }
    let unknown = |reason: String| Provenance::Unknown {
        executable: canonical.clone(),
        reason,
    };
    let layout = match layout::from_executable(&canonical, platform) {
        Ok(layout) => layout,
        Err(_) => {
            return unknown("it is not <install_root>/bin/draft of an official installation".into())
        }
    };
    let receipt = match receipt::read(&layout) {
        Ok(Some(receipt)) => receipt,
        Ok(None) => return unknown("its installation root has no receipt".into()),
        Err(error) => return unknown(error.message),
    };
    if let Err(error) = receipt.validate(&layout, &ReceiptContext::FinalManaged) {
        return unknown(error.message);
    }
    let identity_ok = Identity::of_file(&layout.executable(Executable::Draft))
        .is_ok_and(|identity| identity == receipt.draft_executable.identity());
    if !identity_ok {
        return unknown("the installed draft does not match its receipt".into());
    }
    Provenance::OfficialStandalone {
        layout,
        receipt: Box::new(receipt),
    }
}

/// Refuse anything but an official standalone installation, with the exact
/// remedy for each class.
pub fn require_official(
    provenance: Provenance,
    command: &str,
) -> DraftResult<(InstallLayout, InstallationReceipt)> {
    match provenance {
        Provenance::OfficialStandalone { layout, receipt } => Ok((layout, *receipt)),
        Provenance::PackageManagerOwned {
            manager,
            command: remedy,
        } => Err(fail(
            InstallationFailure::UnsupportedInstallationMethod,
            format!("this Draft is managed by {manager}; `{command}` will not modify it"),
        )
        .with_suggestion(format!("Run: {remedy}"))),
        Provenance::SourceOrDevelopment { executable } => Err(fail(
            InstallationFailure::UnsupportedInstallationMethod,
            format!(
                "{} is a source or development build; `{command}` never touches a source tree",
                executable.display()
            ),
        )
        .with_suggestion("Rebuild from source, or install an official release.")),
        Provenance::Unknown { executable, reason } => {
            Err(unknown_guidance(&executable, &reason, command))
        }
    }
}

/// I16: a receipt-less (legacy or copied) installation is never adopted.
fn unknown_guidance(executable: &Path, reason: &str, command: &str) -> DraftError {
    let remedy = if cfg!(windows) {
        "Re-run install.ps1: it installs into <install_root>\\bin with a receipt, and later \
         updates and uninstalls are managed from there."
            .to_string()
    } else {
        "Remove or rename both copied binaries on your PATH (for example ~/.local/bin/draft and \
         ~/.local/bin/draftd), or set DRAFT_MIGRATE_LEGACY_PATH=1 to authorize replacing them, \
         then run the official install.sh. It installs into a dedicated root and links PATH to it."
            .to_string()
    };
    fail(
        InstallationFailure::UnknownInstallationProvenance,
        format!(
            "`{command}` manages only official installations; {} is not one ({reason})",
            executable.display()
        ),
    )
    .with_suggestion(remedy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installation::receipt::fixtures;

    #[test]
    fn a_development_build_is_never_self_managed() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("target/debug/draft");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"x").unwrap();
        assert!(matches!(
            detect(&exe, InstallPlatform::Unix),
            Provenance::SourceOrDevelopment { .. }
        ));
    }

    #[test]
    fn a_copy_without_a_receipt_is_unknown_with_guidance() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("pathbin/draft");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"x").unwrap();
        let provenance = detect(&exe, InstallPlatform::Unix);
        assert!(matches!(provenance, Provenance::Unknown { .. }));
        let error = require_official(provenance, "draft update").unwrap_err();
        assert!(error
            .suggestion
            .unwrap()
            .contains("DRAFT_MIGRATE_LEGACY_PATH=1"));

        // Even a proper-looking root is Unknown without a valid receipt.
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin/draft"), b"x").unwrap();
        assert!(matches!(
            detect(&root.join("bin/draft"), InstallPlatform::Unix),
            Provenance::Unknown { .. }
        ));
    }

    #[test]
    fn a_receipt_bearing_root_whose_binary_matches_is_official() {
        let dir = tempfile::tempdir().unwrap();
        let root = layout::canonicalize(dir.path()).unwrap().join("root");
        let layout = InstallLayout::new(&root, InstallPlatform::Unix);
        std::fs::create_dir_all(layout.bin_dir()).unwrap();
        std::fs::write(layout.executable(Executable::Draft), b"draft").unwrap();
        std::fs::write(layout.executable(Executable::Draftd), b"draftd").unwrap();
        let receipt = fixtures::unix(&layout, &dir.path().join("pathbin"));
        receipt::write(&layout, &receipt).unwrap();
        assert!(matches!(
            detect(&layout.executable(Executable::Draft), InstallPlatform::Unix),
            Provenance::OfficialStandalone { .. }
        ));
        // A binary that no longer matches its receipt is not.
        std::fs::write(layout.executable(Executable::Draft), b"tampered").unwrap();
        assert!(matches!(
            detect(&layout.executable(Executable::Draft), InstallPlatform::Unix),
            Provenance::Unknown { .. }
        ));
    }
}
