//! What a declarative package may contain, and how its content is hashed.
//!
//! These are format rules about *declared relative paths* and *package
//! content* — pure string and byte logic, with no filesystem access. Draft
//! separately resolves every path against the real filesystem under its own
//! path guard before trusting a package. Both checks run; neither replaces the
//! other, and this crate deliberately owns only the half a publisher outside
//! the Draft repository can run.

use crate::{FormatError, FormatResult};
use sha2::{Digest, Sha256};

/// The manifest, at the package root.
pub const MANIFEST_FILE: &str = "extension.json";

pub const CONTRIBUTIONS_PREFIX: &str = "contributions";
pub const DOCS_PREFIX: &str = "docs";
pub const ASSETS_PREFIX: &str = "assets";
/// Extension-owned result schemas. Their bytes are covered by the package
/// content hash and therefore by the package signature, which is what lets a
/// historical artifact be validated against the exact schema that produced it.
pub const SCHEMAS_PREFIX: &str = "schemas";

pub const CONTRIBUTION_EXTENSIONS: &[&str] = &["json", "toml"];
pub const DOC_EXTENSIONS: &[&str] = &["md", "txt"];
pub const ASSET_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];
pub const SCHEMA_EXTENSIONS: &[&str] = &["json"];

/// Largest single extension-owned schema document.
pub const MAX_SCHEMA_BYTES: u64 = 256 * 1024;
/// Deepest nesting an extension-owned schema may use.
pub const MAX_SCHEMA_DEPTH: u32 = 32;
/// Most nodes an extension-owned schema may contain.
pub const MAX_SCHEMA_NODES: u32 = 8_192;
/// Most local `$ref`s an extension-owned schema may contain.
pub const MAX_SCHEMA_REFS: u32 = 256;

/// Largest package content the format admits.
pub const MAX_PACKAGE_BYTES: u64 = 32 * 1024 * 1024;
/// Largest single file inside a package.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Reject a declared path that is not a plain, forward-slashed, relative path
/// inside the package.
///
/// This is the format rule about what a package may *say*. It is intentionally
/// strict and intentionally not a substitute for Draft's filesystem path guard,
/// which additionally resolves symlinks and confirms containment on disk.
pub fn validate_relative_path(path: &str) -> FormatResult<()> {
    let unsafe_reason = if path.is_empty() {
        Some("is empty")
    } else if path.contains('\0') {
        Some("contains a NUL byte")
    } else if path.contains('\\') {
        Some("uses a backslash separator")
    } else if path.starts_with('/') {
        Some("is absolute")
    } else if path.len() >= 2 && path.as_bytes()[1] == b':' {
        Some("names a drive letter")
    } else if path
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        Some("contains an empty, '.' or '..' segment")
    } else if path.ends_with('/') {
        Some("names a directory")
    } else {
        None
    };
    match unsafe_reason {
        Some(reason) => Err(FormatError::UnsafePath(format!(
            "declared path '{path}' {reason}"
        ))),
        None => Ok(()),
    }
}

/// Validate a declared path's namespace and file type.
///
/// `required_prefix` is the directory the path must sit directly under; an
/// empty prefix means the package root.
pub fn validate_declared_path(
    path: &str,
    required_prefix: &str,
    allowed_extensions: &[&str],
) -> FormatResult<()> {
    validate_relative_path(path)?;
    if required_prefix.is_empty() {
        if path.contains('/') {
            return Err(FormatError::Path(format!(
                "declared path '{path}' must sit at the package root"
            )));
        }
    } else if path.split('/').next() != Some(required_prefix) {
        return Err(FormatError::Path(format!(
            "declared path '{path}' must be under {required_prefix}/"
        )));
    }
    if !allowed_extensions.contains(&extension_of(path)) {
        return Err(FormatError::Path(format!(
            "declared path '{path}' has an unsupported static file type"
        )));
    }
    Ok(())
}

/// License and notice files are named, not merely typed.
pub fn validate_license_name(path: &str) -> FormatResult<()> {
    let name = path
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.starts_with("license") || name.starts_with("notice") {
        Ok(())
    } else {
        Err(FormatError::Path(format!(
            "declared path '{path}' must name a LICENSE or NOTICE file"
        )))
    }
}

/// Whether a package-relative path is one the format admits at all.
///
/// Used when walking a candidate package directory or archive: anything outside
/// this set is refused before it is copied or extracted.
pub fn is_admissible_package_path(path: &str) -> bool {
    if validate_relative_path(path).is_err() {
        return false;
    }
    if path == MANIFEST_FILE {
        return true;
    }
    let extension = extension_of(path);
    match path.split('/').next() {
        Some(CONTRIBUTIONS_PREFIX) => CONTRIBUTION_EXTENSIONS.contains(&extension),
        Some(DOCS_PREFIX) => DOC_EXTENSIONS.contains(&extension),
        Some(ASSETS_PREFIX) => ASSET_EXTENSIONS.contains(&extension),
        Some(SCHEMAS_PREFIX) => {
            SCHEMA_EXTENSIONS.contains(&extension) && path.ends_with(".schema.json")
        }
        _ => {
            !path.contains('/')
                && DOC_EXTENSIONS.contains(&extension)
                && validate_license_name(path).is_ok()
        }
    }
}

/// The canonical byte stream a package's content hash is taken over.
///
/// Entries are `(package-relative path, file bytes)`. Order does not matter:
/// the stream is built from the entries sorted by path, so two identical
/// package trees always hash the same regardless of how they were walked.
pub fn content_stream(entries: &mut [(String, Vec<u8>)]) -> FormatResult<Vec<u8>> {
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut stream = Vec::new();
    for (path, bytes) in entries.iter() {
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(FormatError::Limit(format!(
                "package file '{path}' exceeds the {MAX_FILE_BYTES} byte limit"
            )));
        }
        stream.extend_from_slice(path.as_bytes());
        stream.push(0);
        stream.extend_from_slice(bytes);
        stream.push(0);
        if stream.len() as u64 > MAX_PACKAGE_BYTES {
            return Err(FormatError::Limit(
                "extension package exceeds size limit".into(),
            ));
        }
    }
    Ok(stream)
}

/// The package content hash, in Draft's `sha256:` digest form.
pub fn content_hash(entries: &mut [(String, Vec<u8>)]) -> FormatResult<String> {
    Ok(digest(&content_stream(entries)?))
}

/// Draft's digest form for arbitrary bytes.
pub fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Whether `value` is a well-formed `sha256:` digest.
pub fn is_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn extension_of(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or_default();
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => extension,
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_and_absolute_paths_are_refused() {
        for rejected in [
            "",
            "/etc/passwd",
            "contributions/../../etc/passwd",
            "contributions//task.json",
            "contributions\\task.json",
            "C:/windows",
            "contributions/",
            "contributions/./task.json",
        ] {
            assert!(
                validate_relative_path(rejected).is_err(),
                "{rejected} should be refused"
            );
        }
        validate_relative_path("contributions/task.json").unwrap();
    }

    #[test]
    fn namespaces_and_file_types_are_enforced() {
        validate_declared_path(
            "contributions/task.json",
            CONTRIBUTIONS_PREFIX,
            CONTRIBUTION_EXTENSIONS,
        )
        .unwrap();
        assert!(validate_declared_path(
            "docs/task.json",
            CONTRIBUTIONS_PREFIX,
            CONTRIBUTION_EXTENSIONS
        )
        .is_err());
        assert!(validate_declared_path(
            "contributions/task.sh",
            CONTRIBUTIONS_PREFIX,
            CONTRIBUTION_EXTENSIONS
        )
        .is_err());
        assert!(validate_declared_path("nested/LICENSE.txt", "", DOC_EXTENSIONS).is_err());
        validate_declared_path("LICENSE.txt", "", DOC_EXTENSIONS).unwrap();
    }

    #[test]
    fn only_declarative_content_is_admissible() {
        for admitted in [
            "extension.json",
            "contributions/rules.toml",
            "docs/readme.md",
            "assets/icon.png",
            "LICENSE.txt",
            "NOTICE.md",
        ] {
            assert!(is_admissible_package_path(admitted), "{admitted}");
        }
        for refused in [
            "entrypoint.sh",
            "contributions/run.sh",
            "bin/tool",
            "docs/script.js",
            "assets/payload.wasm",
            "README.md",
            "../escape.json",
        ] {
            assert!(!is_admissible_package_path(refused), "{refused}");
        }
    }

    #[test]
    fn content_hashing_is_order_independent_and_bounded() {
        let mut forward = vec![
            ("a.txt".to_string(), b"one".to_vec()),
            ("b.txt".to_string(), b"two".to_vec()),
        ];
        let mut reversed = vec![
            ("b.txt".to_string(), b"two".to_vec()),
            ("a.txt".to_string(), b"one".to_vec()),
        ];
        assert_eq!(
            content_hash(&mut forward).unwrap(),
            content_hash(&mut reversed).unwrap()
        );
        assert!(is_digest(&content_hash(&mut forward).unwrap()));

        let mut oversized = vec![(
            "big.txt".to_string(),
            vec![0u8; MAX_FILE_BYTES as usize + 1],
        )];
        assert!(matches!(
            content_hash(&mut oversized),
            Err(FormatError::Limit(_))
        ));
    }

    #[test]
    fn digest_shape_is_checked() {
        assert!(is_digest(&digest(b"payload")));
        assert!(!is_digest("sha256:not-hex"));
        assert!(!is_digest(&digest(b"payload")[..70]));
        assert!(!is_digest(&digest(b"payload").to_uppercase()));
    }
}
