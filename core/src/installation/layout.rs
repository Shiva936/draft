//! The fixed slots of one installation, all derived from `<install_root>`.
//!
//! No lifecycle path is ever taken from an argument, a receipt field or a
//! journal field: every mutable or destructive target is re-derived here from
//! the validated root, the fixed executable names and the operation id.

use std::path::{Path, PathBuf};

use super::{fail, InstallPlatform, InstallationFailure, InstallationOperationId};
use crate::support::error::DraftResult;

/// The two executables an installation owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Executable {
    Draft,
    Draftd,
}

impl Executable {
    pub fn stem(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Draftd => "draftd",
        }
    }
}

/// `<install_root>` and every fixed slot beneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallLayout {
    root: PathBuf,
    platform: InstallPlatform,
}

impl InstallLayout {
    pub fn new(root: impl Into<PathBuf>, platform: InstallPlatform) -> Self {
        Self {
            root: root.into(),
            platform,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn platform(&self) -> InstallPlatform {
        self.platform
    }

    pub fn file_name(&self, executable: Executable) -> String {
        format!("{}{}", executable.stem(), self.platform.exe_suffix())
    }

    /// `bin/draft[.exe]` / `bin/draftd[.exe]`, relative to the root.
    pub fn relative(&self, executable: Executable) -> String {
        format!("bin/{}", self.file_name(executable))
    }

    pub fn bin_dir(&self) -> PathBuf {
        self.root.join("bin")
    }

    pub fn executable(&self, executable: Executable) -> PathBuf {
        self.bin_dir().join(self.file_name(executable))
    }

    pub fn lifecycle_dir(&self) -> PathBuf {
        self.root.join(".draft-install")
    }

    pub fn receipt(&self) -> PathBuf {
        self.lifecycle_dir().join("receipt.json")
    }

    /// The permanent, stable lock sidecar. Never removed.
    pub fn lock(&self) -> PathBuf {
        self.lifecycle_dir().join("lifecycle.lock")
    }

    pub fn operation(&self) -> PathBuf {
        self.lifecycle_dir().join("operation.json")
    }

    pub fn bootstrap_record(&self) -> PathBuf {
        self.lifecycle_dir().join("bootstrap.recovery")
    }

    pub fn terminal_record(&self) -> PathBuf {
        self.lifecycle_dir().join("terminal-cleanup")
    }

    pub fn staging_root(&self) -> PathBuf {
        self.lifecycle_dir().join("staging")
    }

    pub fn staging(&self, operation: &InstallationOperationId) -> PathBuf {
        self.staging_root().join(operation.as_str())
    }

    pub fn rollback_root(&self) -> PathBuf {
        self.lifecycle_dir().join("rollback")
    }

    pub fn rollback(&self, operation: &InstallationOperationId) -> PathBuf {
        self.rollback_root().join(operation.as_str())
    }

    /// The surviving uninstall executor: `staging/<operation_id>/draft[.exe]`.
    pub fn helper(&self, operation: &InstallationOperationId) -> PathBuf {
        self.staging(operation)
            .join(self.file_name(Executable::Draft))
    }

    /// A staged target binary during an update or fresh install.
    pub fn staged(&self, operation: &InstallationOperationId, executable: Executable) -> PathBuf {
        self.staging(operation)
            .join("bin")
            .join(self.file_name(executable))
    }

    /// A rollback backup of the previous binary during an update.
    pub fn backup(&self, operation: &InstallationOperationId, executable: Executable) -> PathBuf {
        self.rollback(operation).join(self.file_name(executable))
    }

    /// A legacy PATH executable moved aside during an opt-in migration.
    pub fn legacy_slot(
        &self,
        operation: &InstallationOperationId,
        executable: Executable,
    ) -> PathBuf {
        self.staging(operation)
            .join("legacy")
            .join(executable.stem())
    }

    /// Create the lifecycle skeleton — `<root>/`, `.draft-install/` — with
    /// owner-only permissions. Nothing else.
    pub fn ensure_skeleton(&self) -> DraftResult<()> {
        crate::support::fsutil::ensure_dir(&self.lifecycle_dir())?;
        let _ = crate::support::hidden::restrict_dir(&self.lifecycle_dir(), 0o700);
        Ok(())
    }
}

/// Strip the Windows verbatim prefix `canonicalize` adds (`\\?\C:\…`), so a
/// canonical path compares equal to the path a person typed.
pub fn normalize_canonical(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        if rest.len() >= 2 && rest.as_bytes()[1] == b':' {
            return PathBuf::from(rest);
        }
    }
    path
}

pub fn canonicalize(path: &Path) -> DraftResult<PathBuf> {
    std::fs::canonicalize(path)
        .map(normalize_canonical)
        .map_err(|error| {
            crate::support::error::DraftError::storage(format!(
                "canonicalize {}: {error}",
                path.display()
            ))
        })
}

/// Derive the installation from the running executable.
///
/// The canonical path (the Unix PATH symlink resolved) must be exactly
/// `<install_root>/bin/draft[.exe]`, and `<install_root>` is its parent's
/// parent. A copied PATH executable does not canonicalize into any root, so it
/// is never mistaken for one.
pub fn from_executable(executable: &Path, platform: InstallPlatform) -> DraftResult<InstallLayout> {
    let canonical = canonicalize(executable)?;
    let expected = format!("draft{}", platform.exe_suffix());
    let name_ok = canonical
        .file_name()
        .is_some_and(|name| name.to_string_lossy() == expected);
    let bin = canonical.parent();
    let bin_ok = bin
        .and_then(Path::file_name)
        .is_some_and(|name| name.to_string_lossy() == "bin");
    match (name_ok && bin_ok, bin.and_then(Path::parent)) {
        (true, Some(root)) => Ok(InstallLayout::new(root, platform)),
        _ => Err(fail(
            InstallationFailure::UnknownInstallationProvenance,
            format!(
                "{} is not <install_root>/bin/{expected}, so it is not an official installation",
                canonical.display()
            ),
        )),
    }
}

/// Filesystem roots, home, temp and system directories that can never be an
/// installation root (or be purged): exact matches, compared canonically.
pub fn is_dangerous_root(path: &Path) -> bool {
    let normalized = normalize_canonical(path.to_path_buf());
    if normalized.parent().is_none() {
        return true;
    }
    let text = normalized.to_string_lossy().replace('\\', "/");
    let trimmed = text.trim_end_matches('/');
    // A bare drive root such as `C:`.
    if trimmed.len() == 2 && trimmed.as_bytes()[1] == b':' {
        return true;
    }
    let same = |other: Option<PathBuf>| {
        other
            .map(|other| {
                std::fs::canonicalize(&other)
                    .map(normalize_canonical)
                    .unwrap_or(other)
            })
            .is_some_and(|other| other == normalized)
    };
    if same(crate::project::home::user_home_dir()) || same(Some(std::env::temp_dir())) {
        return true;
    }
    const SYSTEM: &[&str] = &[
        "/bin",
        "/sbin",
        "/boot",
        "/dev",
        "/etc",
        "/lib",
        "/lib64",
        "/proc",
        "/root",
        "/run",
        "/sys",
        "/usr",
        "/usr/bin",
        "/usr/sbin",
        "/usr/lib",
        "/usr/local",
        "/usr/local/bin",
        "/var",
        "/tmp",
        "/opt",
        "/home",
        "/Users",
        "/System",
        "/Library",
        "/Applications",
        "/private",
        "/private/tmp",
        "/private/var",
    ];
    if SYSTEM.contains(&trimmed) {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    const WINDOWS_SYSTEM: &[&str] = &[
        "windows",
        "windows/system32",
        "program files",
        "program files (x86)",
        "programdata",
        "users",
    ];
    if lower.len() > 3 && lower.as_bytes()[1] == b':' {
        let rest = &lower[3..];
        if WINDOWS_SYSTEM.contains(&rest) {
            return true;
        }
    }
    false
}

/// Validate a root the installer selected (`DRAFT_INSTALL_ROOT` or the
/// platform default): absolute, not a UNC or verbatim path, not a dangerous
/// root. Returns the canonical form of the deepest existing ancestor joined
/// with the rest, so a symlinked parent cannot smuggle the root elsewhere.
pub fn validate_selected_root(root: &Path) -> DraftResult<PathBuf> {
    let text = root.to_string_lossy();
    if !root.is_absolute() || text.starts_with(r"\\") {
        return Err(fail(
            InstallationFailure::PermissionDenied,
            format!(
                "installation root {} must be an absolute local path",
                root.display()
            ),
        ));
    }
    let mut existing = root.to_path_buf();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut resolved = canonicalize(&existing)?;
    for component in rest.into_iter().rev() {
        resolved.push(component);
    }
    if is_dangerous_root(&resolved) {
        return Err(fail(
            InstallationFailure::PermissionDenied,
            format!(
                "{} can never be a Draft installation root",
                resolved.display()
            ),
        ));
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_slot_derives_from_the_root_and_the_operation_id() {
        let layout = InstallLayout::new("/opt/draft-root", InstallPlatform::Windows);
        let op = InstallationOperationId::new("ilo_0123456789ab");
        assert_eq!(layout.relative(Executable::Draft), "bin/draft.exe");
        assert_eq!(
            layout.helper(&op),
            Path::new("/opt/draft-root/.draft-install/staging/ilo_0123456789ab/draft.exe")
        );
        assert_eq!(
            layout.lock(),
            Path::new("/opt/draft-root/.draft-install/lifecycle.lock")
        );
    }

    #[test]
    fn only_the_canonical_bin_slot_is_an_installation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin/draft"), b"x").unwrap();
        let layout = from_executable(&root.join("bin/draft"), InstallPlatform::Unix).unwrap();
        assert_eq!(layout.root(), canonicalize(&root).unwrap());

        // A copy elsewhere is not an installation.
        std::fs::create_dir_all(dir.path().join("pathbin")).unwrap();
        std::fs::write(dir.path().join("pathbin/draft"), b"x").unwrap();
        assert!(from_executable(&dir.path().join("pathbin/draft"), InstallPlatform::Unix).is_err());

        // A symlink into the root resolves to it.
        #[cfg(unix)]
        {
            let link = dir.path().join("pathbin/draft-link");
            std::os::unix::fs::symlink(root.join("bin/draft"), &link).unwrap();
            assert_eq!(
                from_executable(&link, InstallPlatform::Unix)
                    .unwrap()
                    .root(),
                canonicalize(&root).unwrap()
            );
        }
    }

    #[test]
    fn dangerous_roots_are_refused() {
        assert!(is_dangerous_root(Path::new("/")));
        assert!(is_dangerous_root(Path::new("/usr/local/bin")));
        assert!(is_dangerous_root(&std::env::temp_dir()));
        if let Some(home) = crate::project::home::user_home_dir() {
            assert!(is_dangerous_root(&home));
        }
        assert!(is_dangerous_root(Path::new("C:")));
        assert!(is_dangerous_root(Path::new("C:/Windows")));
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_dangerous_root(&dir.path().join("root")));
        assert!(validate_selected_root(Path::new("relative/root")).is_err());
        assert!(validate_selected_root(&dir.path().join("a/b")).is_ok());
    }
}
