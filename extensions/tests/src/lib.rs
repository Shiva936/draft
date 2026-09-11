//! Conformance checks for the official Draft extension packages.
//!
//! These run with no Draft platform present: no core crate, no services, no
//! daemon, no store. The only Draft dependency is the portable
//! `draft-extension-contract` crate, which is exactly the position a standalone
//! `draft-extensions` repository would be in.
//!
//! What they assert is that every shipped package is a package: it validates
//! against the declarative format, contains only what the format admits,
//! declares a permission only when it actually needs one, and describes itself
//! well enough for the catalog metadata to be derived from it rather than
//! written twice.

use draft_extension_contract::{package, ExtensionManifest};
use std::path::{Path, PathBuf};

/// Where the shipped packages live, relative to this crate.
pub fn packages_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tests/ sits inside the extensions workspace")
        .join("packages")
}

/// One package as it sits on disk.
pub struct Package {
    pub root: PathBuf,
    pub manifest: ExtensionManifest,
}

/// Every official package, sorted by id.
pub fn packages() -> Vec<Package> {
    let mut found: Vec<Package> = std::fs::read_dir(packages_dir())
        .expect("the packages directory is readable")
        .filter_map(|entry| {
            let root = entry.ok()?.path();
            if !root.is_dir() {
                return None;
            }
            let bytes = std::fs::read(root.join(package::MANIFEST_FILE)).ok()?;
            let manifest: ExtensionManifest = serde_json::from_slice(&bytes)
                .unwrap_or_else(|error| panic!("{}: {error}", root.display()));
            Some(Package { root, manifest })
        })
        .collect();
    found.sort_by(|left, right| left.manifest.id.cmp(&right.manifest.id));
    found
}

/// The Draft API version a manifest's requirement is checked against.
pub fn declared_api(manifest: &ExtensionManifest) -> String {
    manifest
        .draft_api
        .trim_start_matches(['^', '~', '=', '>', '<', ' '])
        .split_whitespace()
        .next()
        .expect("a manifest declares a usable draft_api")
        .to_string()
}

/// Every file in a package, as package-relative paths.
pub fn package_files(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("package directory is readable") {
            let entry = entry.expect("package entry is readable");
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.push(
                    path.strip_prefix(root)
                        .expect("path is inside the package")
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use draft_extension_contract::{ContributionPayload, ExtensionPermission, PresentationEngineId};
    use serde_json::Value;

    #[test]
    fn every_official_package_validates_against_the_format() {
        let packages = packages();
        assert!(!packages.is_empty(), "there are official packages to check");
        for package in &packages {
            package
                .manifest
                .validate_shape(&declared_api(&package.manifest))
                .unwrap_or_else(|error| panic!("{}: {error}", package.root.display()));
        }
    }

    #[test]
    fn every_declared_path_exists_and_nothing_undeclared_is_shipped() {
        for entry in packages() {
            for declared in entry.manifest.declared_paths() {
                assert!(
                    entry.root.join(declared).is_file(),
                    "{}: declares '{declared}' but does not ship it",
                    entry.root.display()
                );
            }
            for relative in package_files(&entry.root) {
                assert!(
                    package::is_admissible_package_path(&relative),
                    "{}: ships '{relative}', which a declarative package may not hold",
                    entry.root.display()
                );
            }
        }
    }

    #[test]
    fn no_package_ships_anything_executable() {
        for entry in packages() {
            for relative in package_files(&entry.root) {
                let forbidden = [
                    ".sh", ".bash", ".py", ".js", ".mjs", ".rb", ".exe", ".dll", ".so", ".dylib",
                    ".wasm", ".bat", ".ps1",
                ];
                assert!(
                    !forbidden.iter().any(|suffix| relative.ends_with(suffix)),
                    "{}: ships '{relative}', which is executable content",
                    entry.root.display()
                );
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = std::fs::metadata(entry.root.join(&relative))
                        .expect("package file is readable")
                        .permissions()
                        .mode();
                    assert_eq!(
                        mode & 0o111,
                        0,
                        "{}: '{relative}' carries an executable permission bit",
                        entry.root.display()
                    );
                }
            }
        }
    }

    #[test]
    fn every_contribution_payload_decodes_and_validates() {
        for entry in packages() {
            for contribution in &entry.manifest.contributions {
                let bytes = std::fs::read(entry.root.join(&contribution.path))
                    .expect("a declared contribution file is readable");
                let payload = ContributionPayload::decode(contribution.kind, &bytes)
                    .unwrap_or_else(|error| {
                        panic!(
                            "{}: contribution '{}' does not decode as {:?}: {error}",
                            entry.root.display(),
                            contribution.id,
                            contribution.kind
                        )
                    });
                payload.validate().unwrap_or_else(|error| {
                    panic!(
                        "{}: contribution '{}' is invalid: {error}",
                        entry.root.display(),
                        contribution.id
                    )
                });
            }
        }
    }

    #[test]
    fn a_text_editor_presentation_names_the_grammar_it_wants() {
        // The Console owns what a grammar *is*; a package says which one its
        // resources want. A `text_editor` binding that names none leaves the
        // Console with nothing to act on and silently renders plain text, which
        // looks identical to "no extension installed" and hides the mistake.
        for entry in packages() {
            for contribution in &entry.manifest.contributions {
                let bytes = std::fs::read(entry.root.join(&contribution.path))
                    .expect("a declared contribution file is readable");
                let Ok(ContributionPayload::Presentation(presentation)) =
                    ContributionPayload::decode(contribution.kind, &bytes)
                else {
                    continue;
                };
                if presentation.engine != PresentationEngineId::TextEditor {
                    continue;
                }
                let grammar = presentation.config.get("grammar").and_then(Value::as_str);
                assert!(
                    grammar.is_some_and(|name| !name.is_empty()),
                    "{}: presentation '{}' uses the text editor without naming a grammar",
                    entry.root.display(),
                    presentation.presentation_id.qualified()
                );
            }
        }
    }

    #[test]
    fn a_package_requests_process_execute_exactly_when_it_declares_a_command() {
        for entry in packages() {
            let declares_command = entry.manifest.contributions.iter().any(|contribution| {
                let bytes = std::fs::read(entry.root.join(&contribution.path))
                    .expect("a declared contribution file is readable");
                ContributionPayload::decode(contribution.kind, &bytes)
                    .map(|payload| !payload.commands().is_empty())
                    .unwrap_or(false)
            });
            let requests_execute = entry
                .manifest
                .permissions
                .contains(&ExtensionPermission::ProcessExecute);
            assert_eq!(
                declares_command,
                requests_execute,
                "{}: a package must request process.execute exactly when it declares a command",
                entry.root.display()
            );
        }
    }

    #[test]
    fn every_package_describes_itself_well_enough_to_be_found() {
        for entry in packages() {
            // Catalog search metadata is derived from these, so a package that
            // omits them would be unfindable by anything but its exact id.
            assert!(
                entry
                    .manifest
                    .description
                    .as_ref()
                    .is_some_and(|text| !text.trim().is_empty()),
                "{}: needs a description for catalog search",
                entry.root.display()
            );
            assert!(
                !entry.manifest.keywords.is_empty(),
                "{}: needs at least one keyword for catalog search",
                entry.root.display()
            );
            assert!(
                !entry.manifest.capabilities().is_empty(),
                "{}: contributes nothing",
                entry.root.display()
            );
        }
    }

    #[test]
    fn official_packages_share_one_publisher_and_id_namespace() {
        for entry in packages() {
            assert_eq!(
                entry.manifest.publisher,
                "draft",
                "{}: official packages are published by draft",
                entry.root.display()
            );
            assert!(
                entry.manifest.id.as_str().starts_with("draft."),
                "{}: official package ids live under the draft. namespace",
                entry.root.display()
            );
        }
    }

    #[test]
    fn no_signing_key_material_is_committed_beside_the_packages() {
        // Production signing keys belong to an authorized signing environment.
        // Nothing in this repository should ever hold one.
        fn scan(dir: &Path) {
            for entry in std::fs::read_dir(dir).expect("directory is readable") {
                let path = entry.expect("entry is readable").path();
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if name == "target" {
                    continue;
                }
                if path.is_dir() {
                    scan(&path);
                    continue;
                }
                let suspicious = [".pem", ".key", "id_ed25519", ".pk8", ".jwk"];
                assert!(
                    !suspicious.iter().any(|marker| name.contains(marker)),
                    "{}: looks like committed key material",
                    path.display()
                );
                if let Ok(text) = std::fs::read_to_string(&path) {
                    // Assembled at runtime so this file does not trip its own
                    // scan by containing the marker it looks for.
                    let marker = ["PRIVATE", "KEY"].join(" ");
                    assert!(
                        !text.contains(&marker),
                        "{}: contains private key material",
                        path.display()
                    );
                }
            }
        }
        scan(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("the extensions workspace root"),
        );
    }
}
