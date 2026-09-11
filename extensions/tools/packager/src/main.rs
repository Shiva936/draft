//! Build and sign official Draft extension artifacts and catalogs.
//!
//! The pipeline is deliberately split so that packaging and trust-root
//! ownership are separate jobs:
//!
//! ```text
//! validate  →  package  →  metadata  →  [ authorized signing environment ]  →  verify
//! ```
//!
//! `validate`, `package` and `metadata` need no key material at all. `sign`
//! takes a key from a file the caller supplies. Nothing here ever generates,
//! stores or commits a production signing key, and ordinary package generation
//! can never create a new root of trust by accident.
//!
//! Its only Draft dependency is the portable `draft-extension-contract` crate, the public
//! boundary. That is what makes this tool — and the packages beside it — a
//! relocation away from being their own repository.

use draft_extension_contract::{
    catalog::{
        signable_bytes, CatalogKey, CatalogTarget, MetadataDescriptor, RoleSpec, RootMetadata,
        SignatureRecord, SignedEnvelope, SnapshotMetadata, TargetsMetadata, TimestampMetadata,
        REQUIRED_ROLES, SIGNATURE_ALGORITHM,
    },
    package, ContributionPayload, ExtensionManifest, FORMAT_REVISION,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod signing;

const USAGE: &str = "\
draft-extension-packager — build and sign official Draft extension artifacts

USAGE:
    draft-extension-packager validate <packages-dir>
    draft-extension-packager build <packages-dir> <out-dir> --catalog-id <id> [--expires <rfc3339>]
    draft-extension-packager sign <out-dir> --key <file> --key-id <id> [--catalog-id <id>]
    draft-extension-packager verify <out-dir>

COMMANDS:
    validate  Check every package against the declarative format rules.
    build     Package each extension and write unsigned, signable catalog roles.
    sign      Sign the catalog roles with externally supplied key material.
    verify    Re-verify a signed catalog end to end.

Signing keys are never generated or stored by this tool. `--key` names a file
holding a base64 Ed25519 signing key, supplied by an authorized signing
environment.
";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let result = match arguments.first().map(String::as_str) {
        Some("validate") => validate(&arguments[1..]),
        Some("build") => build(&arguments[1..]),
        Some("sign") => sign(&arguments[1..]),
        Some("verify") => verify(&arguments[1..]),
        Some("--help") | Some("-h") | None => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(other) => Err(format!("unknown command '{other}'\n\n{USAGE}")),
    };
    match result {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

// --- validate ---------------------------------------------------------------

fn validate(arguments: &[String]) -> Result<String, String> {
    let packages_dir = positional(arguments, 0, "packages-dir")?;
    let packages = read_packages(&packages_dir)?;
    for (path, manifest) in &packages {
        validate_package(path, manifest)?;
    }
    Ok(format!("{} package(s) validated", packages.len()))
}

/// Every rule the format applies to a package, plus the filesystem half.
fn validate_package(root: &Path, manifest: &ExtensionManifest) -> Result<(), String> {
    manifest
        .validate_shape(draft_api_version(manifest)?)
        .map_err(|error| format!("{}: {error}", root.display()))?;

    // Every declared path must really be there …
    for declared in manifest.declared_paths() {
        if !root.join(declared).is_file() {
            return Err(format!(
                "{}: declares '{declared}', which is not a file in the package",
                root.display()
            ));
        }
    }
    // … and nothing may be there that the format does not admit.
    for (relative, _) in collect_files(root)? {
        if !package::is_admissible_package_path(&relative) {
            return Err(format!(
                "{}: contains '{relative}', which a declarative package may not hold",
                root.display()
            ));
        }
    }

    // Every contribution must decode, satisfy its own rules, and mint only
    // identifiers this package owns. Deferring any of that to install time
    // means the failure surfaces as a capability that silently never appears,
    // which is the hardest kind to diagnose.
    for contribution in &manifest.contributions {
        let path = root.join(&contribution.path);
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("{}: cannot read '{}': {error}", root.display(), contribution.path))?;
        let payload = ContributionPayload::decode(contribution.kind, &bytes)
            .map_err(|error| format!("{}: '{}' is not a valid {:?} contribution: {error}", root.display(), contribution.path, contribution.kind))?;
        payload
            .validate()
            .map_err(|error| format!("{}: '{}' is invalid: {error}", root.display(), contribution.path))?;
        for id in payload.contributed_ids() {
            if !id.is_owned_by(manifest.id.as_str()) {
                return Err(format!(
                    "{}: '{}' mints '{id}', which is not namespaced to the declaring extension '{}'",
                    root.display(),
                    contribution.path,
                    manifest.id
                ));
            }
        }
    }
    Ok(())
}

/// The API version a manifest's requirement is checked against.
///
/// Taken from the manifest's own requirement so the tool needs no Draft build
/// to validate against: `^0.3.4` is checked against `0.3.4`.
fn draft_api_version(manifest: &ExtensionManifest) -> Result<&str, String> {
    manifest
        .draft_api
        .trim_start_matches(['^', '~', '=', '>', '<', ' '])
        .split_whitespace()
        .next()
        .ok_or_else(|| {
            format!(
                "manifest declares an unusable draft_api '{}'",
                manifest.draft_api
            )
        })
}

// --- build ------------------------------------------------------------------

fn build(arguments: &[String]) -> Result<String, String> {
    let packages_dir = positional(arguments, 0, "packages-dir")?;
    let out_dir = positional(arguments, 1, "out-dir")?;
    let catalog_id = flag(arguments, "--catalog-id").ok_or("build requires --catalog-id")?;
    let expires =
        flag(arguments, "--expires").unwrap_or_else(|| "2099-01-01T00:00:00Z".to_string());

    let packages = read_packages(&packages_dir)?;
    if packages.is_empty() {
        return Err(format!(
            "no packages found under {}",
            packages_dir.display()
        ));
    }
    std::fs::create_dir_all(out_dir.join("artifacts")).map_err(io)?;

    let mut targets = Vec::new();
    for (root, manifest) in &packages {
        validate_package(root, manifest)?;
        let artifact = archive(root)?;
        let artifact_path = format!("artifacts/{}.tar", manifest.id);
        std::fs::write(out_dir.join(&artifact_path), &artifact).map_err(io)?;

        // Search metadata is derived from the validated manifest, never
        // authored a second time, so a catalog cannot describe a package
        // differently from how the package describes itself.
        targets.push(CatalogTarget {
            id: manifest.id.to_string(),
            version: manifest.version.clone(),
            publisher: manifest.publisher.clone(),
            draft_api: manifest.draft_api.clone(),
            artifact_path,
            length: artifact.len() as u64,
            sha256: package::digest(&artifact),
            name: Some(manifest.name.clone()),
            description: manifest.description.clone(),
            keywords: manifest.keywords.clone(),
            capabilities: manifest.capabilities(),
        });
    }
    targets.sort_by(|left, right| left.id.cmp(&right.id));

    let targets_role = TargetsMetadata {
        schema_version: FORMAT_REVISION,
        catalog_id: catalog_id.clone(),
        role: "targets".into(),
        version: 1,
        expires_at: expires,
        packages: targets,
        delegations: Vec::new(),
    };

    // Only the targets role can be built without signatures. Snapshot pins the
    // bytes of the signed targets document and timestamp pins the signed
    // snapshot, so both are produced by `sign`.
    write_json(&out_dir.join("unsigned/targets.json"), &targets_role)?;

    Ok(format!(
        "packaged {} extension(s) and wrote the signable targets role to {}/unsigned",
        packages.len(),
        out_dir.display()
    ))
}

fn descriptor(version: u64, bytes: &[u8]) -> MetadataDescriptor {
    MetadataDescriptor {
        version,
        length: bytes.len() as u64,
        sha256: package::digest(bytes),
    }
}

// --- sign -------------------------------------------------------------------

fn sign(arguments: &[String]) -> Result<String, String> {
    let out_dir = positional(arguments, 0, "out-dir")?;
    let key_file = flag(arguments, "--key").ok_or(
        "sign requires --key naming a file with base64 Ed25519 key material. \
         This tool never generates a production key.",
    )?;
    let key_id = flag(arguments, "--key-id").ok_or("sign requires --key-id")?;

    let key = signing::SigningKey::from_file(Path::new(&key_file))?;
    let catalog_id = flag(arguments, "--catalog-id");

    let targets: TargetsMetadata = read_json(&out_dir.join("unsigned/targets.json"))?;
    let catalog_id = catalog_id.unwrap_or_else(|| targets.catalog_id.clone());
    let expires_at = targets.expires_at.clone();

    let role_spec = RoleSpec {
        key_ids: vec![key_id.clone()],
        threshold: 1,
    };
    let root = RootMetadata {
        schema_version: FORMAT_REVISION,
        catalog_id: catalog_id.clone(),
        role: "root".into(),
        version: 1,
        expires_at: expires_at.clone(),
        keys: BTreeMap::from([(
            key_id.clone(),
            CatalogKey {
                algorithm: SIGNATURE_ALGORITHM.into(),
                public_key: key.public_key_base64(),
            },
        )]),
        roles: REQUIRED_ROLES
            .iter()
            .map(|role| ((*role).to_string(), role_spec.clone()))
            .collect(),
        revoked_key_ids: Vec::new(),
        revoked_packages: Vec::new(),
    };
    root.validate_shape().map_err(|error| error.to_string())?;

    // Sign upward: each role's descriptor pins the exact bytes published for
    // the role beneath it, which is what a consumer fetches and hashes.
    let targets_bytes = envelope_bytes(&targets, &key, &key_id)?;
    std::fs::write(out_dir.join("targets.json"), &targets_bytes).map_err(io)?;

    let snapshot = SnapshotMetadata {
        schema_version: FORMAT_REVISION,
        catalog_id: catalog_id.clone(),
        role: "snapshot".into(),
        version: 1,
        expires_at: expires_at.clone(),
        roles: BTreeMap::from([(
            "targets".to_string(),
            descriptor(targets.version, &targets_bytes),
        )]),
    };
    let snapshot_bytes = envelope_bytes(&snapshot, &key, &key_id)?;
    std::fs::write(out_dir.join("snapshot.json"), &snapshot_bytes).map_err(io)?;

    let timestamp = TimestampMetadata {
        schema_version: FORMAT_REVISION,
        catalog_id,
        role: "timestamp".into(),
        version: 1,
        expires_at,
        snapshot: descriptor(snapshot.version, &snapshot_bytes),
    };
    let timestamp_bytes = envelope_bytes(&timestamp, &key, &key_id)?;
    std::fs::write(out_dir.join("timestamp.json"), &timestamp_bytes).map_err(io)?;

    let root_bytes = envelope_bytes(&root, &key, &key_id)?;
    std::fs::write(out_dir.join("root.json"), &root_bytes).map_err(io)?;

    Ok(format!(
        "signed catalog in {}\ntrust anchor (root fingerprint): {}",
        out_dir.display(),
        package::digest(&root_bytes)
    ))
}

/// Serialize one role as a signed envelope, exactly as it will be published.
///
/// These are the bytes a consumer fetches and hashes, so they are also the
/// bytes the role above must pin.
fn envelope_bytes<T: serde::Serialize + Clone>(
    role: &T,
    key: &signing::SigningKey,
    key_id: &str,
) -> Result<Vec<u8>, String> {
    let message = signable_bytes(role).map_err(|error| error.to_string())?;
    let envelope = SignedEnvelope {
        signed: role.clone(),
        signatures: vec![SignatureRecord {
            key_id: key_id.to_string(),
            signature: key.sign_base64(&message),
        }],
    };
    serde_json::to_vec_pretty(&envelope).map_err(|error| error.to_string())
}

// --- verify -----------------------------------------------------------------

fn verify(arguments: &[String]) -> Result<String, String> {
    let out_dir = positional(arguments, 0, "out-dir")?;
    let root: SignedEnvelope<RootMetadata> = read_json(&out_dir.join("root.json"))?;
    root.signed
        .validate_shape()
        .map_err(|error| error.to_string())?;

    let check = |role: &str, envelope_bytes: &[u8]| -> Result<(), String> {
        let spec = root
            .signed
            .roles
            .get(role)
            .ok_or_else(|| format!("root does not authorize role '{role}'"))?;
        let envelope: SignedEnvelope<serde_json::Value> =
            serde_json::from_slice(envelope_bytes).map_err(|error| error.to_string())?;
        draft_extension_contract::catalog::verify_envelope(
            &envelope,
            spec,
            &root.signed.keys,
            &root.signed.revoked_key_ids,
        )
        .map_err(|error| format!("{role}: {error}"))
    };

    for role in REQUIRED_ROLES {
        let bytes = std::fs::read(out_dir.join(format!("{role}.json"))).map_err(io)?;
        check(role, &bytes)?;
    }

    // Every published artifact must still hash to what targets says it does.
    let targets: SignedEnvelope<TargetsMetadata> = read_json(&out_dir.join("targets.json"))?;
    for target in &targets.signed.packages {
        let artifact = std::fs::read(out_dir.join(&target.artifact_path)).map_err(io)?;
        if package::digest(&artifact) != target.sha256 || artifact.len() as u64 != target.length {
            return Err(format!(
                "artifact for '{}' does not match its signed digest or length",
                target.id
            ));
        }
    }
    Ok(format!(
        "verified {} signed package(s) in {}",
        targets.signed.packages.len(),
        out_dir.display()
    ))
}

// --- shared helpers ---------------------------------------------------------

fn read_packages(dir: &Path) -> Result<Vec<(PathBuf, ExtensionManifest)>, String> {
    let mut packages = Vec::new();
    let entries = std::fs::read_dir(dir)
        .map_err(|error| format!("cannot read {}: {error}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(io)?.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join(package::MANIFEST_FILE);
        if !manifest_path.is_file() {
            continue;
        }
        let manifest: ExtensionManifest = read_json(&manifest_path)?;
        packages.push((path, manifest));
    }
    packages.sort_by(|left, right| left.1.id.cmp(&right.1.id));
    Ok(packages)
}

/// Package contents, ordered so the archive is byte-reproducible.
fn collect_files(root: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(io)? {
            let entry = entry.map_err(io)?;
            let path = entry.path();
            let kind = entry.file_type().map_err(io)?;
            if kind.is_symlink() {
                return Err(format!(
                    "{}: packages may not contain symlinks",
                    path.display()
                ));
            }
            if kind.is_dir() {
                walk(root, &path, out)?;
            } else if kind.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((relative, std::fs::read(&path).map_err(io)?));
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

/// A deterministic tar of the package, so the same input always digests alike.
fn archive(root: &Path) -> Result<Vec<u8>, String> {
    let mut builder = tar::Builder::new(Vec::new());
    for (relative, bytes) in collect_files(root)? {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder
            .append_data(&mut header, &relative, bytes.as_slice())
            .map_err(io)?;
    }
    builder.into_inner().map_err(io)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes =
        std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    std::fs::write(path, bytes).map_err(io)
}

fn positional(arguments: &[String], index: usize, name: &str) -> Result<PathBuf, String> {
    arguments
        .iter()
        .filter(|argument| !argument.starts_with("--"))
        .nth(index)
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing <{name}>\n\n{USAGE}"))
}

fn flag(arguments: &[String], name: &str) -> Option<String> {
    arguments
        .iter()
        .position(|argument| argument == name)
        .and_then(|index| arguments.get(index + 1))
        .cloned()
}

fn io(error: impl std::fmt::Display) -> String {
    error.to_string()
}
