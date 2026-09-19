//! Release archives: verified before opening, extracted under hard bounds.
//!
//! The expected artifact is `draft-v<version>-<target>.tar.gz` (`.zip` on
//! Windows) containing exactly `draft-v<version>-<target>/bin/{draft,draftd}`
//! plus `README.md`, `LICENSE` and optional `NOTICE`. Draft's own packaging
//! emits explicit directory entries for the package directory and its `bin/`,
//! so those two — and only those two — are accepted; every other entry must be
//! a regular file. Symlinks, hardlinks, devices, fifos, absolute paths,
//! traversal, entries outside the prefix and duplicates are refused, and
//! extraction only ever writes under `staging/<operation_id>/`.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use super::layout::Executable;
use super::release::ManifestArtifact;
use super::{fail, Identity, InstallPlatform, InstallationFailure};
use crate::support::error::{DraftError, DraftResult};

pub const MAX_ARCHIVE_ENTRIES: usize = 64;
pub const MAX_ARCHIVE_UNCOMPRESSED_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_ARCHIVE_ENTRY_BYTES: u64 = 256 * 1024 * 1024;

/// The frozen package name for a version and target.
pub fn package_name(version: &str, target: &str) -> String {
    format!("draft-v{version}-{target}")
}

pub fn artifact_name(version: &str, target: &str) -> String {
    let extension = if target.contains("windows") {
        "zip"
    } else {
        "tar.gz"
    };
    format!("{}.{extension}", package_name(version, target))
}

fn invalid(why: impl Into<String>) -> DraftError {
    fail(InstallationFailure::ArchiveInvalid, why)
}

/// Check a downloaded artifact against its manifest entry before it is opened.
pub fn verify(path: &Path, expected: &ManifestArtifact) -> DraftResult<()> {
    let actual = Identity::of_file(path)?;
    if actual.size != expected.size || actual.sha256 != expected.sha256 {
        return Err(fail(
            InstallationFailure::DigestMismatch,
            format!(
                "{} does not match the signed release manifest",
                expected.asset
            ),
        ));
    }
    Ok(())
}

/// The two binaries an extraction produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedPair {
    pub draft: PathBuf,
    pub draftd: PathBuf,
}

enum Entry {
    Directory,
    File,
    Other,
}

/// Classify one entry's normalized path under the package prefix.
struct Rules {
    prefix: String,
    draft: String,
    draftd: String,
    seen: BTreeSet<String>,
    total: u64,
    count: usize,
}

impl Rules {
    fn new(prefix: &str, platform: InstallPlatform) -> Self {
        Self {
            prefix: prefix.to_string(),
            draft: format!("bin/draft{}", platform.exe_suffix()),
            draftd: format!("bin/draftd{}", platform.exe_suffix()),
            seen: BTreeSet::new(),
            total: 0,
            count: 0,
        }
    }

    /// Returns the path relative to the package directory for a file that
    /// should be written, or `None` for an accepted directory / ignored file.
    fn admit(&mut self, raw: &str, kind: Entry, size: u64) -> DraftResult<Option<String>> {
        self.count += 1;
        if self.count > MAX_ARCHIVE_ENTRIES {
            return Err(invalid(format!(
                "more than {MAX_ARCHIVE_ENTRIES} archive entries"
            )));
        }
        if raw.starts_with('/') || raw.starts_with('\\') || raw.contains('\0') {
            return Err(invalid(format!("absolute or malformed entry {raw:?}")));
        }
        let trimmed = raw.trim_end_matches('/');
        let normalized = crate::support::pathguard::check_relative(trimmed)
            .map_err(|violation| invalid(format!("unsafe entry {raw:?}: {violation:?}")))?;
        if !self.seen.insert(normalized.clone()) {
            return Err(invalid(format!("duplicate entry {normalized}")));
        }
        let inside = if normalized == self.prefix {
            Some("")
        } else {
            normalized.strip_prefix(&format!("{}/", self.prefix))
        };
        let Some(inside) = inside else {
            return Err(invalid(format!("{normalized} is outside {}/", self.prefix)));
        };
        match kind {
            Entry::Directory => {
                if (inside.is_empty() || inside == "bin") && size == 0 {
                    Ok(None)
                } else {
                    Err(invalid(format!("unexpected directory {normalized}")))
                }
            }
            Entry::Other => Err(invalid(format!(
                "{normalized} is a link, device, fifo or other special entry"
            ))),
            Entry::File => {
                if size > MAX_ARCHIVE_ENTRY_BYTES {
                    return Err(invalid(format!("{normalized} exceeds the entry size cap")));
                }
                self.total += size;
                if self.total > MAX_ARCHIVE_UNCOMPRESSED_BYTES {
                    return Err(invalid("the archive exceeds its uncompressed size cap"));
                }
                if inside == self.draft || inside == self.draftd {
                    Ok(Some(inside.to_string()))
                } else if ["README.md", "LICENSE", "NOTICE"].contains(&inside) {
                    Ok(None)
                } else {
                    Err(invalid(format!("unexpected file {normalized}")))
                }
            }
        }
    }
}

/// Stream one entry into a create-new file, bounded by its cap even if the
/// header lied about its size.
fn write_bounded(mut reader: impl Read, target: &Path, total: &mut u64) -> DraftResult<()> {
    if let Some(parent) = target.parent() {
        crate::support::fsutil::ensure_dir(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| DraftError::storage(format!("create {}: {error}", target.display())))?;
    let mut limited = (&mut reader).take(MAX_ARCHIVE_ENTRY_BYTES + 1);
    let mut buffer = vec![0u8; 64 * 1024];
    let mut written = 0u64;
    loop {
        let read = limited
            .read(&mut buffer)
            .map_err(|error| invalid(format!("read archive entry: {error}")))?;
        if read == 0 {
            break;
        }
        written += read as u64;
        *total += read as u64;
        if written > MAX_ARCHIVE_ENTRY_BYTES || *total > MAX_ARCHIVE_UNCOMPRESSED_BYTES {
            return Err(invalid("decompressed data exceeds its cap"));
        }
        file.write_all(&buffer[..read])
            .map_err(|error| DraftError::storage(format!("write {}: {error}", target.display())))?;
    }
    file.sync_all()
        .map_err(|error| DraftError::storage(format!("sync {}: {error}", target.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| DraftError::storage(format!("chmod {}: {error}", target.display())))?;
    }
    Ok(())
}

/// Extract `archive` into `dest`, returning the two binaries.
pub fn extract(
    archive: &Path,
    version: &str,
    target: &str,
    dest: &Path,
    platform: InstallPlatform,
) -> DraftResult<ExtractedPair> {
    let prefix = package_name(version, target);
    let mut rules = Rules::new(&prefix, platform);
    let mut decompressed = 0u64;
    let mut written = BTreeSet::new();
    let file = std::fs::File::open(archive)
        .map_err(|error| DraftError::storage(format!("open {}: {error}", archive.display())))?;
    if archive.to_string_lossy().ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(file).map_err(|error| invalid(format!("{error}")))?;
        if zip.len() > MAX_ARCHIVE_ENTRIES {
            return Err(invalid(format!(
                "more than {MAX_ARCHIVE_ENTRIES} archive entries"
            )));
        }
        for index in 0..zip.len() {
            let entry = zip
                .by_index(index)
                .map_err(|error| invalid(format!("{error}")))?;
            let name = entry.name().replace('\\', "/");
            let kind = if entry.is_symlink() {
                Entry::Other
            } else if entry.is_dir() {
                Entry::Directory
            } else if entry.is_file() {
                Entry::File
            } else {
                Entry::Other
            };
            let size = if entry.is_dir() { 0 } else { entry.size() };
            if let Some(relative) = rules.admit(&name, kind, size)? {
                write_bounded(entry, &dest.join(&relative), &mut decompressed)?;
                written.insert(relative);
            }
        }
    } else {
        let decoder = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(decoder);
        let entries = tar.entries().map_err(|error| invalid(format!("{error}")))?;
        for entry in entries {
            let entry = entry.map_err(|error| invalid(format!("{error}")))?;
            let header = entry.header();
            let name = String::from_utf8(entry.path_bytes().into_owned())
                .map_err(|_| invalid("an entry name is not UTF-8"))?;
            let kind = match header.entry_type() {
                tar::EntryType::Regular | tar::EntryType::Continuous => Entry::File,
                tar::EntryType::Directory => Entry::Directory,
                _ => Entry::Other,
            };
            let size = header.size().map_err(|error| invalid(format!("{error}")))?;
            if let Some(relative) = rules.admit(&name, kind, size)? {
                write_bounded(entry, &dest.join(&relative), &mut decompressed)?;
                written.insert(relative);
            }
        }
    }
    let draft = format!("bin/draft{}", platform.exe_suffix());
    let draftd = format!("bin/draftd{}", platform.exe_suffix());
    for required in [&draft, &draftd] {
        if !written.contains(required.as_str()) {
            return Err(invalid(format!("the archive has no {prefix}/{required}")));
        }
    }
    Ok(ExtractedPair {
        draft: dest.join(draft),
        draftd: dest.join(draftd),
    })
}

/// The installed-name of an extracted binary, for staging.
pub fn extracted(pair: &ExtractedPair, executable: Executable) -> &Path {
    match executable {
        Executable::Draft => &pair.draft,
        Executable::Draftd => &pair.draftd,
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// A `.tar.gz` built entry by entry, including hostile ones.
    pub fn tar_gz(entries: &[(&str, &[u8], tar::EntryType)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, data, kind) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(*kind);
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            {
                let raw = header.as_old_mut();
                let bytes = name.as_bytes();
                raw.name[..bytes.len()].copy_from_slice(bytes);
            }
            header.set_cksum();
            builder.append(&header, *data).unwrap();
        }
        let tar = builder.into_inner().unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&tar).unwrap();
        encoder.finish().unwrap()
    }

    /// A well-formed package for `version`/`target` holding `draft` / `draftd`.
    pub fn package(version: &str, target: &str, draft: &[u8], draftd: &[u8]) -> Vec<u8> {
        let prefix = package_name(version, target);
        let dir = tar::EntryType::Directory;
        let file = tar::EntryType::Regular;
        let names = [
            format!("{prefix}/"),
            format!("{prefix}/bin/"),
            format!("{prefix}/bin/draft"),
            format!("{prefix}/bin/draftd"),
            format!("{prefix}/README.md"),
            format!("{prefix}/LICENSE"),
        ];
        tar_gz(&[
            (&names[0], b"", dir),
            (&names[1], b"", dir),
            (&names[2], draft, file),
            (&names[3], draftd, file),
            (&names[4], b"readme", file),
            (&names[5], b"license", file),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use tar::EntryType as T;

    const V: &str = "0.3.4";
    const TARGET: &str = "x86_64-unknown-linux-musl";

    fn extract_bytes(bytes: &[u8]) -> DraftResult<ExtractedPair> {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join(artifact_name(V, TARGET));
        std::fs::write(&archive, bytes).unwrap();
        let out = dir.path().join("out");
        let result = extract(&archive, V, TARGET, &out, InstallPlatform::Unix);
        // Nothing is ever written outside the destination.
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            assert!(name == "out" || name.ends_with(".tar.gz"), "{name}");
        }
        std::mem::forget(dir);
        result
    }

    #[test]
    fn a_well_formed_package_with_its_directory_entries_is_accepted() {
        let pair = extract_bytes(&package(V, TARGET, b"draft", b"draftd")).unwrap();
        assert_eq!(std::fs::read(&pair.draft).unwrap(), b"draft");
        assert_eq!(std::fs::read(&pair.draftd).unwrap(), b"draftd");
    }

    #[test]
    fn every_unsafe_or_unexpected_entry_is_refused() {
        let p = package_name(V, TARGET);
        let file = |name: String| (name, b"x".to_vec(), T::Regular);
        let base = || {
            vec![
                (format!("{p}/"), Vec::new(), T::Directory),
                file(format!("{p}/bin/draft")),
                file(format!("{p}/bin/draftd")),
            ]
        };
        let mut cases: Vec<Vec<(String, Vec<u8>, T)>> = Vec::new();
        cases.push(base()[..2].to_vec()); // missing draftd
        cases.push(vec![base()[0].clone(), base()[2].clone()]); // missing draft
        let mut traversal = base();
        traversal.push(file(format!("{p}/../escape")));
        cases.push(traversal);
        let mut absolute = base();
        absolute.push(file("/etc/passwd".into()));
        cases.push(absolute);
        for kind in [T::Symlink, T::Link, T::Char, T::Block, T::Fifo] {
            let mut special = base();
            special.push((format!("{p}/bin/extra"), Vec::new(), kind));
            cases.push(special);
        }
        let mut duplicate = base();
        duplicate.push(file(format!("{p}/bin/draft")));
        cases.push(duplicate);
        let mut outside_dir = base();
        outside_dir.push(("elsewhere/".into(), Vec::new(), T::Directory));
        cases.push(outside_dir);
        let mut extra_dir = base();
        extra_dir.push((format!("{p}/share/"), Vec::new(), T::Directory));
        cases.push(extra_dir);
        let mut extra_exe = base();
        extra_exe.push(file(format!("{p}/bin/sh")));
        cases.push(extra_exe);
        cases.push(vec![
            file("draft-v9.9.9-x86_64-unknown-linux-musl/bin/draft".into()),
            file("draft-v9.9.9-x86_64-unknown-linux-musl/bin/draftd".into()),
        ]);
        let mut many = base();
        for i in 0..MAX_ARCHIVE_ENTRIES {
            many.push(file(format!("{p}/README.md{i}")));
        }
        cases.push(many);
        for case in cases {
            let entries: Vec<(&str, &[u8], T)> = case
                .iter()
                .map(|(n, d, k)| (n.as_str(), d.as_slice(), *k))
                .collect();
            assert!(extract_bytes(&tar_gz(&entries)).is_err(), "{case:?}");
        }
    }

    #[test]
    fn a_header_that_lies_about_its_size_cannot_exceed_the_cap() {
        let p = package_name(V, TARGET);
        // The claimed size is over the entry cap: refused before reading.
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(T::Regular);
        header.set_size(MAX_ARCHIVE_ENTRY_BYTES + 1);
        header.set_path(format!("{p}/bin/draft")).unwrap();
        header.set_cksum();
        let mut tar = header.as_bytes().to_vec();
        tar.extend_from_slice(&[0u8; 1024]);
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&tar).unwrap();
        assert!(extract_bytes(&encoder.finish().unwrap()).is_err());
    }

    #[test]
    fn the_digest_and_size_are_checked_before_opening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.tar.gz");
        std::fs::write(&path, b"bytes").unwrap();
        let identity = Identity::of_file(&path).unwrap();
        let mut expected = ManifestArtifact {
            target: TARGET.into(),
            asset: "a.tar.gz".into(),
            sha256: identity.sha256.clone(),
            size: identity.size,
        };
        verify(&path, &expected).unwrap();
        expected.size += 1;
        assert!(verify(&path, &expected).is_err());
    }

    /// The archive Draft's own release packaging produces is accepted,
    /// directory entries included — by running `scripts/package-release.sh`.
    #[cfg(unix)]
    #[test]
    fn an_artifact_from_the_real_packaging_script_is_accepted() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let fake = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(fake.path().join("scripts")).unwrap();
        std::fs::copy(
            repo.join("scripts/package-release.sh"),
            fake.path().join("scripts/package-release.sh"),
        )
        .unwrap();
        let release = fake.path().join(format!("target/{TARGET}/release"));
        std::fs::create_dir_all(&release).unwrap();
        std::fs::write(release.join("draft"), b"packaged draft").unwrap();
        std::fs::write(release.join("draftd"), b"packaged draftd").unwrap();
        for file in ["README.md", "LICENSE", "NOTICE"] {
            std::fs::write(fake.path().join(file), file).unwrap();
        }
        let status = std::process::Command::new("bash")
            .arg(fake.path().join("scripts/package-release.sh"))
            .args([V, TARGET, "dist"])
            .status()
            .unwrap();
        assert!(status.success());
        let archive = fake.path().join("dist").join(artifact_name(V, TARGET));
        let out = fake.path().join("out");
        let pair = extract(&archive, V, TARGET, &out, InstallPlatform::Unix).unwrap();
        assert_eq!(std::fs::read(pair.draft).unwrap(), b"packaged draft");
        assert_eq!(std::fs::read(pair.draftd).unwrap(), b"packaged draftd");
    }
}
