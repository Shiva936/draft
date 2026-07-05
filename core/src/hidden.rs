//! Cross-platform hidden-directory handling for `.draft/` (PRD §9.2, TDD §8).
//!
//! On Unix/macOS the leading `.` already hides the directory; we additionally
//! tighten permissions where the store holds secrets. On Windows the dot is not
//! enough, so we set `FILE_ATTRIBUTE_HIDDEN` explicitly and report failures so
//! `draft doctor` can surface them.

use std::path::Path;

/// Outcome of attempting to mark a directory hidden, so `doctor` can report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HiddenStatus {
    /// The directory is hidden by virtue of its `.`-prefixed name (Unix/macOS).
    DotPrefixed,
    /// The Windows hidden attribute is set.
    AttributeSet,
    /// The attribute could not be set; carries a human-readable reason.
    Failed(String),
}

impl HiddenStatus {
    pub fn is_ok(&self) -> bool {
        !matches!(self, HiddenStatus::Failed(_))
    }
}

/// Ensure `dir` is hidden for the current platform. Idempotent.
pub fn ensure_hidden(dir: &Path) -> HiddenStatus {
    #[cfg(windows)]
    {
        set_windows_hidden(dir)
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        HiddenStatus::DotPrefixed
    }
}

/// Apply restrictive permissions to a directory that stores secrets
/// (`~/.draft/keys`, mode 0700). No-op where the platform lacks Unix modes.
pub fn restrict_dir(dir: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(dir)?.permissions();
        perms.set_mode(mode);
        std::fs::set_permissions(dir, perms)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (dir, mode);
        Ok(())
    }
}

/// Apply restrictive permissions to a secret file (private signing key, 0600).
pub fn restrict_file(file: &Path, mode: u32) -> std::io::Result<()> {
    restrict_dir(file, mode)
}

/// Report whether a directory is currently hidden (best-effort, for `doctor`).
pub fn is_hidden(dir: &Path) -> bool {
    #[cfg(windows)]
    {
        windows_is_hidden(dir).unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        dir.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with('.'))
            .unwrap_or(false)
    }
}

#[cfg(windows)]
fn set_windows_hidden(dir: &Path) -> HiddenStatus {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileAttributesW, SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, INVALID_FILE_ATTRIBUTES,
    };
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let attrs = GetFileAttributesW(wide.as_ptr());
        if attrs == INVALID_FILE_ATTRIBUTES {
            return HiddenStatus::Failed(format!("cannot read attributes of {}", dir.display()));
        }
        if SetFileAttributesW(wide.as_ptr(), attrs | FILE_ATTRIBUTE_HIDDEN) == 0 {
            return HiddenStatus::Failed(format!(
                "SetFileAttributesW failed for {}",
                dir.display()
            ));
        }
    }
    HiddenStatus::AttributeSet
}

#[cfg(windows)]
fn windows_is_hidden(dir: &Path) -> Option<bool> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, INVALID_FILE_ATTRIBUTES,
    };
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let attrs = GetFileAttributesW(wide.as_ptr());
        if attrs == INVALID_FILE_ATTRIBUTES {
            return None;
        }
        Some(attrs & FILE_ATTRIBUTE_HIDDEN != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_hidden_is_ok_for_dot_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".draft");
        std::fs::create_dir(&dir).unwrap();
        assert!(ensure_hidden(&dir).is_ok());
        assert!(is_hidden(&dir));
    }
}
