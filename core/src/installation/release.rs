//! Release resolution and self-update trust (stage 2).
//!
//! *Resolution*: the default target is the maximum SemVer among releases
//! eligible on the selected channel, found by a bounded paged scan of
//! `GET /repos/<repo>/releases` — never GitHub's ordering, never publication
//! dates, never `/releases/latest`. A scan that hits its bound with a full
//! final page has not seen every release, so it fails rather than choosing
//! from an incomplete set. `--version` is an exact tag lookup.
//!
//! *Trust*: `release-manifest.json`'s exact published bytes carry a detached
//! Ed25519 signature, verified before the manifest is parsed, under a key from
//! the set embedded in this binary. When the canonical signature has moved to
//! a key this build does not know, the per-key overlap signatures are tried in
//! embedded order. A dormant client that trusts no current key updates
//! *through* a signed bridge release whose manifest declares the new key.
//!
//! The first-run installers are a different trust stage (HTTPS + `SHA256SUMS`)
//! and never verify `release-manifest.sig`.

use serde::{Deserialize, Serialize};

use super::receipt::ReleaseChannel;
use super::{fail, InstallationFailure};
use crate::support::error::DraftResult;

pub const RELEASE_PAGE_SIZE: usize = 100;
pub const MAX_RELEASE_PAGES: usize = 10;
pub const MAX_RELEASES_SCANNED: usize = 1000;
pub const MAX_RELEASE_MANIFEST_BYTES: u64 = 1024 * 1024;
pub const MAX_RELEASE_SIGNATURE_BYTES: u64 = 8 * 1024;
pub const MAX_RELEASE_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_TRUSTED_RELEASE_KEYS: usize = 4;
pub const MAX_RELEASE_TRUST_HOPS: u32 = 2;
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
/// The detached-signature envelope beside the manifest.
pub const SIGNATURE_SCHEMA_VERSION: u32 = 1;
pub const MANIFEST_ASSET: &str = "release-manifest.json";
pub const SIGNATURE_ASSET: &str = "release-manifest.sig";

/// The release-verification public keys embedded in this binary, in their
/// frozen order: `(key_id, base64 Ed25519 public key)`.
///
/// Defined unconditionally — never behind a `cfg` — so the packaged binary's
/// `release-trust-set --json` always reports exactly what it trusts. It is
/// empty until the maintainer provisions the protected release signing key;
/// with no trusted key, `draft update` fails closed with
/// `ReleaseSigningKeyUnknown` and never installs an unverified artifact.
pub const RELEASE_TRUSTED_KEYS: &[(&str, &str)] = &[];

/// One release as the enumeration sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub tag: String,
    pub draft: bool,
    pub prerelease: bool,
    pub published: bool,
}

/// Where releases come from. The real one is HTTPS to the repository's
/// release host; tests inject one, so no test touches the network.
pub trait ReleaseSource {
    /// One page of `GET /repos/<repo>/releases?per_page=&page=` (1-based).
    fn list(&self, page: usize, per_page: usize) -> DraftResult<Vec<ReleaseInfo>>;
    /// `GET /repos/<repo>/releases/tags/<tag>`; `None` when there is none.
    fn by_tag(&self, tag: &str) -> DraftResult<Option<ReleaseInfo>>;
    /// A release asset, bounded; `None` for a 404. Exceeding `cap` is an error.
    fn fetch(&self, tag: &str, asset: &str, cap: u64) -> DraftResult<Option<Vec<u8>>>;
    /// Stream a release asset into `dest` (create-new), aborting as soon as
    /// `cap` is exceeded. `false` for a 404.
    fn fetch_to(
        &self,
        tag: &str,
        asset: &str,
        cap: u64,
        dest: &std::path::Path,
    ) -> DraftResult<bool> {
        match self.fetch(tag, asset, cap)? {
            Some(bytes) => {
                if bytes.len() as u64 > cap {
                    return Err(fail(
                        InstallationFailure::ReleaseArtifactTooLarge,
                        format!("{asset} exceeds {cap} bytes"),
                    ));
                }
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(dest)
                    .map_err(|error| {
                        crate::support::error::DraftError::storage(format!(
                            "create {}: {error}",
                            dest.display()
                        ))
                    })?;
                file.write_all(&bytes)
                    .and_then(|()| file.sync_all())
                    .map_err(|error| {
                        crate::support::error::DraftError::storage(error.to_string())
                    })?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

/// `v<semver>` exactly.
pub fn parse_tag(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.strip_prefix('v')?).ok()
}

/// I17: stable considers only stable releases; the prerelease track considers
/// stable *and* prerelease ones, so it is never stranded below stable.
pub fn is_eligible(release: &ReleaseInfo, channel: ReleaseChannel) -> Option<semver::Version> {
    if release.draft || !release.published {
        return None;
    }
    let version = parse_tag(&release.tag)?;
    let prerelease = release.prerelease || !version.pre.is_empty();
    match channel {
        ReleaseChannel::Stable if prerelease => None,
        _ => Some(version),
    }
}

/// Every eligible release, by a bounded scan to exhaustion.
pub fn enumerate(
    source: &dyn ReleaseSource,
    channel: ReleaseChannel,
) -> DraftResult<Vec<(semver::Version, ReleaseInfo)>> {
    let mut eligible = Vec::new();
    let mut scanned = 0;
    for page in 1..=MAX_RELEASE_PAGES {
        let releases = source.list(page, RELEASE_PAGE_SIZE)?;
        scanned += releases.len();
        let exhausted = releases.len() < RELEASE_PAGE_SIZE;
        for release in releases {
            if let Some(version) = is_eligible(&release, channel) {
                eligible.push((version, release));
            }
        }
        if exhausted {
            return Ok(eligible);
        }
        if scanned >= MAX_RELEASES_SCANNED {
            break;
        }
    }
    Err(fail(
        InstallationFailure::ReleaseEnumerationLimitExceeded,
        format!(
            "the release history was not scanned to exhaustion within {MAX_RELEASES_SCANNED} \
             releases, so no maximum can be proven"
        ),
    ))
}

/// The maximum eligible SemVer, or `None` when nothing is eligible.
pub fn newest(
    source: &dyn ReleaseSource,
    channel: ReleaseChannel,
) -> DraftResult<Option<(semver::Version, ReleaseInfo)>> {
    Ok(enumerate(source, channel)?
        .into_iter()
        .max_by(|(a, _), (b, _)| a.cmp(b)))
}

/// `--version <semver>`: the exact tag, regardless of channel.
pub fn exact(source: &dyn ReleaseSource, version: &semver::Version) -> DraftResult<ReleaseInfo> {
    let tag = format!("v{version}");
    let release = source.by_tag(&tag)?.ok_or_else(|| {
        fail(
            InstallationFailure::ReleaseUnavailable,
            format!("Draft {version} has not been released"),
        )
    })?;
    if release.draft || !release.published || release.tag != tag {
        return Err(fail(
            InstallationFailure::ReleaseUnavailable,
            format!("Draft {version} is not a published release"),
        ));
    }
    Ok(release)
}

/// One artifact the manifest vouches for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestArtifact {
    pub target: String,
    pub asset: String,
    pub sha256: String,
    pub size: u64,
}

/// `release-manifest.json`. `channel` classifies *this release*; it is not the
/// installation's track, and a prerelease-track install may consume a
/// stable-classified manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub version: String,
    pub channel: String,
    pub tag: String,
    /// Exactly the release-verification key ids embedded in this release's
    /// binaries, in embedded order — proven equal at release time.
    pub trusted_key_ids: Vec<String>,
    pub artifacts: Vec<ManifestArtifact>,
    pub generated_at: String,
}

impl ReleaseManifest {
    pub fn artifact_for(&self, target: &str) -> DraftResult<&ManifestArtifact> {
        let mut matching = self
            .artifacts
            .iter()
            .filter(|artifact| artifact.target == target);
        match (matching.next(), matching.next()) {
            (Some(artifact), None) => Ok(artifact),
            _ => Err(fail(
                InstallationFailure::ReleaseMetadataInvalid,
                format!(
                    "release {} does not name exactly one {target} artifact",
                    self.tag
                ),
            )),
        }
    }
}

/// `release-manifest.sig` / `release-manifest.<key_id>.sig`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureEnvelope {
    pub schema_version: u32,
    pub key_id: String,
    pub algorithm: String,
    pub signature: String,
}

/// `rk_` + the first 12 hex of sha256(public key).
pub fn key_id_of(public_key: &[u8; 32]) -> String {
    let digest = <sha2::Sha256 as sha2::Digest>::digest(public_key);
    format!("rk_{}", &crate::support::hashing::hex_encode(&digest)[..12])
}

/// A validated, ordered trusted key set.
#[derive(Clone)]
pub struct TrustSet {
    keys: Vec<(String, ed25519_dalek::VerifyingKey)>,
}

impl TrustSet {
    /// Build and guard a set: every declared id equals the id derived from its
    /// key, ids are unique, and there are at most `MAX_TRUSTED_RELEASE_KEYS`.
    pub fn new(entries: &[(&str, &str)]) -> DraftResult<Self> {
        use base64::Engine;
        let collision = |why: String| fail(InstallationFailure::TrustedReleaseKeyCollision, why);
        if entries.len() > MAX_TRUSTED_RELEASE_KEYS {
            return Err(collision(format!(
                "at most {MAX_TRUSTED_RELEASE_KEYS} release keys may be trusted"
            )));
        }
        let mut keys: Vec<(String, ed25519_dalek::VerifyingKey)> = Vec::new();
        for (declared, encoded) in entries {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .ok()
                .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
                .ok_or_else(|| collision(format!("{declared} is not a 32-byte base64 key")))?;
            let derived = key_id_of(&bytes);
            if derived != *declared {
                return Err(collision(format!("{declared} derives as {derived}")));
            }
            if keys.iter().any(|(id, _)| id == declared) {
                return Err(collision(format!("{declared} appears twice")));
            }
            let key = ed25519_dalek::VerifyingKey::from_bytes(&bytes)
                .map_err(|error| collision(format!("{declared}: {error}")))?;
            keys.push((declared.to_string(), key));
        }
        Ok(Self { keys })
    }

    /// The set this binary embeds.
    pub fn embedded() -> DraftResult<Self> {
        Self::new(RELEASE_TRUSTED_KEYS)
    }

    pub fn ids(&self) -> Vec<String> {
        self.keys.iter().map(|(id, _)| id.clone()).collect()
    }

    fn get(&self, id: &str) -> Option<&ed25519_dalek::VerifyingKey> {
        self.keys
            .iter()
            .find(|(key_id, _)| key_id == id)
            .map(|(_, key)| key)
    }

    /// Whether `declared` adds at least one key this set lacks.
    pub fn expands_with(&self, declared: &[String]) -> bool {
        declared.iter().any(|id| self.get(id).is_none())
    }
}

fn signature_invalid(why: impl Into<String>) -> crate::support::error::DraftError {
    fail(InstallationFailure::ReleaseSignatureInvalid, why)
}

/// Verify one envelope over the exact manifest bytes with `key`.
fn verify_envelope(
    bytes: &[u8],
    envelope_bytes: &[u8],
    requested: Option<&str>,
    trust: &TrustSet,
) -> Option<Result<String, crate::support::error::DraftError>> {
    use base64::Engine;
    let envelope: SignatureEnvelope = match serde_json::from_slice(envelope_bytes) {
        Ok(envelope) => envelope,
        Err(error) => {
            return Some(Err(signature_invalid(format!(
                "malformed signature: {error}"
            ))))
        }
    };
    if requested.is_some_and(|requested| requested != envelope.key_id) {
        // A per-key file naming another key is rejected, never retried.
        return Some(Err(signature_invalid(format!(
            "a signature filed under {} names {}",
            requested.unwrap_or_default(),
            envelope.key_id
        ))));
    }
    let key = trust.get(&envelope.key_id)?;
    if envelope.schema_version != SIGNATURE_SCHEMA_VERSION || envelope.algorithm != "ed25519" {
        return Some(Err(signature_invalid("unsupported signature envelope")));
    }
    let Some(signature) = base64::engine::general_purpose::STANDARD
        .decode(&envelope.signature)
        .ok()
        .and_then(|raw| <[u8; 64]>::try_from(raw).ok())
    else {
        return Some(Err(signature_invalid(
            "the signature is not 64 bytes of base64",
        )));
    };
    let signature = ed25519_dalek::Signature::from_bytes(&signature);
    Some(
        key.verify_strict(bytes, &signature)
            .map(|()| envelope.key_id.clone())
            .map_err(|_| signature_invalid("the manifest signature does not verify")),
    )
}

/// A release whose manifest verified under a trusted key.
#[derive(Debug, Clone)]
pub struct VerifiedRelease {
    pub version: semver::Version,
    pub tag: String,
    pub manifest: ReleaseManifest,
    pub key_id: String,
}

/// Fetch and verify a release manifest: bounded bytes → signature over those
/// exact bytes → only then parse. At most `1 + trusted keys` signature
/// requests.
pub fn verify_release(
    source: &dyn ReleaseSource,
    tag: &str,
    trust: &TrustSet,
) -> DraftResult<VerifiedRelease> {
    let too_large = |asset: &str, cap: u64| {
        let kind = if asset == MANIFEST_ASSET {
            InstallationFailure::ReleaseManifestTooLarge
        } else {
            InstallationFailure::ReleaseMetadataInvalid
        };
        fail(kind, format!("{asset} exceeds {cap} bytes"))
    };
    let bytes = source
        .fetch(tag, MANIFEST_ASSET, MAX_RELEASE_MANIFEST_BYTES)?
        .ok_or_else(|| {
            fail(
                InstallationFailure::ReleaseMetadataInvalid,
                format!("release {tag} publishes no {MANIFEST_ASSET}"),
            )
        })?;
    if bytes.len() as u64 > MAX_RELEASE_MANIFEST_BYTES {
        return Err(too_large(MANIFEST_ASSET, MAX_RELEASE_MANIFEST_BYTES));
    }
    let mut fetched_any = false;
    let mut failure = None;
    let mut verified = None;
    if let Some(envelope) = source.fetch(tag, SIGNATURE_ASSET, MAX_RELEASE_SIGNATURE_BYTES)? {
        fetched_any = true;
        match verify_envelope(&bytes, &envelope, None, trust) {
            Some(Ok(key_id)) => verified = Some(key_id),
            Some(Err(error)) => failure = Some(error),
            None => {}
        }
    }
    if verified.is_none() {
        for key_id in trust.ids() {
            let asset = format!("release-manifest.{key_id}.sig");
            let Some(envelope) = source.fetch(tag, &asset, MAX_RELEASE_SIGNATURE_BYTES)? else {
                continue;
            };
            fetched_any = true;
            match verify_envelope(&bytes, &envelope, Some(&key_id), trust) {
                Some(Ok(id)) => {
                    verified = Some(id);
                    break;
                }
                Some(Err(error)) => failure = Some(error),
                None => {}
            }
        }
    }
    let Some(key_id) = verified else {
        return Err(failure.unwrap_or_else(|| {
            if fetched_any {
                fail(
                    InstallationFailure::ReleaseSigningKeyUnknown,
                    format!("release {tag} is signed only by keys this Draft does not trust"),
                )
            } else {
                fail(
                    InstallationFailure::ReleaseSigningKeyUnknown,
                    format!("release {tag} publishes no signature this Draft can check"),
                )
            }
        }));
    };
    let manifest: ReleaseManifest = serde_json::from_slice(&bytes).map_err(|error| {
        fail(
            InstallationFailure::ReleaseMetadataInvalid,
            format!("{MANIFEST_ASSET} is malformed: {error}"),
        )
    })?;
    let version = parse_tag(tag).ok_or_else(|| {
        fail(
            InstallationFailure::ReleaseMetadataInvalid,
            format!("{tag} is not v<semver>"),
        )
    })?;
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION
        || manifest.tag != tag
        || manifest.version != version.to_string()
        || !["stable", "prerelease"].contains(&manifest.channel.as_str())
    {
        return Err(fail(
            InstallationFailure::ReleaseMetadataInvalid,
            format!("the manifest for {tag} does not describe {tag}"),
        ));
    }
    Ok(VerifiedRelease {
        version,
        tag: tag.to_string(),
        manifest,
        key_id,
    })
}

/// What `draft update` (or `--check`) should do.
#[derive(Debug, Clone)]
pub enum Plan {
    /// The installed version is already the target.
    UpToDate { target: semver::Version },
    /// Install the verified target directly.
    Install(VerifiedRelease),
    /// Install this bridge first, then continue from the new binary.
    Bridge {
        bridge: VerifiedRelease,
        target: semver::Version,
    },
}

/// Find the highest bridge `B` with `installed < B <= target`, eligible on the
/// channel, verifiable now, declaring at least one key we lack, and with an
/// artifact for this platform. Trust expansion is proven from signed metadata
/// alone — no untrusted binary is ever run to inspect its keys.
pub fn find_bridge(
    source: &dyn ReleaseSource,
    candidates: &[(semver::Version, ReleaseInfo)],
    installed: &semver::Version,
    target: &semver::Version,
    trust: &TrustSet,
    platform_target: &str,
) -> DraftResult<Option<VerifiedRelease>> {
    let mut ordered: Vec<_> = candidates
        .iter()
        .filter(|(version, _)| version > installed && version <= target)
        .collect();
    ordered.sort_by(|(a, _), (b, _)| b.cmp(a));
    for (_, release) in ordered {
        let Ok(verified) = verify_release(source, &release.tag, trust) else {
            continue;
        };
        if trust.expands_with(&verified.manifest.trusted_key_ids)
            && verified.manifest.artifact_for(platform_target).is_ok()
        {
            return Ok(Some(verified));
        }
    }
    Ok(None)
}

/// Resolve what to install. `pinned` is `--version`; otherwise the channel's
/// maximum eligible SemVer. `hops` counts bridge re-executions so far.
pub fn plan(
    source: &dyn ReleaseSource,
    trust: &TrustSet,
    installed: &semver::Version,
    channel: ReleaseChannel,
    pinned: Option<&semver::Version>,
    platform_target: &str,
    hops: u32,
) -> DraftResult<Plan> {
    let target_info = match pinned {
        Some(version) => exact(source, version)?,
        None => newest(source, channel)?
            .map(|(_, release)| release)
            .ok_or_else(|| {
                fail(
                    InstallationFailure::ReleaseUnavailable,
                    format!("no {} release is published", channel.as_str()),
                )
            })?,
    };
    let target = parse_tag(&target_info.tag).expect("eligible tags parse");
    if &target == installed {
        return Ok(Plan::UpToDate { target });
    }
    match verify_release(source, &target_info.tag, trust) {
        Ok(verified) => {
            verified.manifest.artifact_for(platform_target)?;
            Ok(Plan::Install(verified))
        }
        Err(error)
            if matches!(
                super::failure_of(&error),
                Some(InstallationFailure::ReleaseSigningKeyUnknown)
                    | Some(InstallationFailure::ReleaseSignatureInvalid)
            ) =>
        {
            if hops >= MAX_RELEASE_TRUST_HOPS {
                return Err(fail(
                    InstallationFailure::ReleaseTrustHopLimitExceeded,
                    format!(
                        "reaching Draft {target} needs more than {MAX_RELEASE_TRUST_HOPS} \
                         trust-bridge updates; reinstall with the official installer"
                    ),
                ));
            }
            let candidates = enumerate(source, channel)?;
            match find_bridge(
                source,
                &candidates,
                installed,
                &target,
                trust,
                platform_target,
            )? {
                Some(bridge) => Ok(Plan::Bridge { bridge, target }),
                None => Err(fail(
                    InstallationFailure::ReleaseTrustBridgeUnavailable,
                    format!(
                        "Draft {installed} cannot verify Draft {target}: this installation is \
                         below the self-update trust floor"
                    ),
                )
                .with_suggestion(
                    "Reinstall with the official installer (install.sh / install.ps1).",
                )),
            }
        }
        Err(error) => Err(error),
    }
}

/// I53, release time: the manifest's `trusted_key_ids` must equal, in order,
/// the set every packaged binary reports — neither subset direction passes.
pub fn check_trust_set_equality(
    manifest: &[String],
    per_target: &[(String, Vec<String>)],
) -> DraftResult<()> {
    let mismatch = |why: String| fail(InstallationFailure::ReleaseTrustSetMismatch, why);
    let mut unique = manifest.to_vec();
    unique.sort();
    unique.dedup();
    if unique.len() != manifest.len() {
        return Err(mismatch("the manifest lists a key id twice".into()));
    }
    for (target, embedded) in per_target {
        if embedded != manifest {
            return Err(mismatch(format!(
                "{target} embeds {embedded:?}, the manifest declares {manifest:?}"
            )));
        }
    }
    Ok(())
}

/// A published release as the retirement gate sees it.
#[derive(Debug, Clone)]
pub struct PublishedRelease {
    pub version: semver::Version,
    pub stable: bool,
    pub signed_by: Vec<String>,
    pub trusted_key_ids: Vec<String>,
}

/// I52, release time: a new release may stop carrying `retiring`'s signature
/// only once a still-published **stable** bridge exists that `retiring`
/// signed and whose manifest declares `successor`.
pub fn check_retirement_bridge(
    published: &[PublishedRelease],
    retiring: &str,
    successor: &str,
) -> DraftResult<()> {
    let bridged = published.iter().any(|release| {
        release.stable
            && release.signed_by.iter().any(|key| key == retiring)
            && release.trusted_key_ids.iter().any(|key| key == successor)
    });
    if bridged {
        Ok(())
    } else {
        Err(fail(
            InstallationFailure::ReleaseRetirementBridgeInvalid,
            format!(
                "no published stable release signed by {retiring} declares {successor}; \
                 {retiring}'s signature cannot be retired yet"
            ),
        ))
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! A fake release host with per-test keys. No test touches the network.
    use super::*;
    use base64::Engine;
    use ed25519_dalek::Signer;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    pub struct Key {
        pub signing: ed25519_dalek::SigningKey,
        pub id: String,
        pub public: String,
    }

    impl Key {
        pub fn new(seed: u8) -> Self {
            let signing = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
            let bytes = signing.verifying_key().to_bytes();
            Self {
                id: key_id_of(&bytes),
                public: base64::engine::general_purpose::STANDARD.encode(bytes),
                signing,
            }
        }

        pub fn envelope(&self, bytes: &[u8]) -> Vec<u8> {
            serde_json::to_vec(&SignatureEnvelope {
                schema_version: SIGNATURE_SCHEMA_VERSION,
                key_id: self.id.clone(),
                algorithm: "ed25519".into(),
                signature: base64::engine::general_purpose::STANDARD
                    .encode(self.signing.sign(bytes).to_bytes()),
            })
            .unwrap()
        }
    }

    pub fn trust(keys: &[&Key]) -> TrustSet {
        let entries: Vec<(&str, &str)> = keys
            .iter()
            .map(|key| (key.id.as_str(), key.public.as_str()))
            .collect();
        TrustSet::new(&entries).unwrap()
    }

    #[derive(Default)]
    pub struct FakeHost {
        pub releases: Vec<ReleaseInfo>,
        pub assets: BTreeMap<(String, String), Vec<u8>>,
        pub fetches: RefCell<Vec<String>>,
    }

    impl FakeHost {
        /// Publish `version` with a manifest signed canonically by `canonical`
        /// and per-key by `overlap`, declaring `declares` as its trust set.
        pub fn publish(
            &mut self,
            version: &str,
            prerelease: bool,
            canonical: &Key,
            overlap: &[&Key],
            declares: &[&Key],
            artifacts: Vec<ManifestArtifact>,
        ) {
            let tag = format!("v{version}");
            self.releases.push(ReleaseInfo {
                tag: tag.clone(),
                draft: false,
                prerelease,
                published: true,
            });
            let manifest = ReleaseManifest {
                schema_version: MANIFEST_SCHEMA_VERSION,
                version: version.into(),
                channel: if prerelease { "prerelease" } else { "stable" }.into(),
                tag: tag.clone(),
                trusted_key_ids: declares.iter().map(|key| key.id.clone()).collect(),
                artifacts,
                generated_at: "2026-09-18T00:00:00Z".into(),
            };
            let bytes = serde_json::to_vec_pretty(&manifest).unwrap();
            self.assets.insert(
                (tag.clone(), SIGNATURE_ASSET.into()),
                canonical.envelope(&bytes),
            );
            for key in overlap {
                self.assets.insert(
                    (tag.clone(), format!("release-manifest.{}.sig", key.id)),
                    key.envelope(&bytes),
                );
            }
            self.assets.insert((tag, MANIFEST_ASSET.into()), bytes);
        }
    }

    impl ReleaseSource for FakeHost {
        fn list(&self, page: usize, per_page: usize) -> DraftResult<Vec<ReleaseInfo>> {
            Ok(self
                .releases
                .iter()
                .skip((page - 1) * per_page)
                .take(per_page)
                .cloned()
                .collect())
        }

        fn by_tag(&self, tag: &str) -> DraftResult<Option<ReleaseInfo>> {
            Ok(self
                .releases
                .iter()
                .find(|release| release.tag == tag)
                .cloned())
        }

        fn fetch(&self, tag: &str, asset: &str, cap: u64) -> DraftResult<Option<Vec<u8>>> {
            self.fetches.borrow_mut().push(asset.to_string());
            match self.assets.get(&(tag.to_string(), asset.to_string())) {
                Some(bytes) if bytes.len() as u64 > cap => Err(fail(
                    if asset == MANIFEST_ASSET {
                        InstallationFailure::ReleaseManifestTooLarge
                    } else {
                        InstallationFailure::ReleaseArtifactTooLarge
                    },
                    format!("{asset} exceeds its cap"),
                )),
                Some(bytes) => Ok(Some(bytes.clone())),
                None => Ok(None),
            }
        }
    }

    pub fn artifact(target: &str) -> ManifestArtifact {
        ManifestArtifact {
            target: target.into(),
            asset: format!("draft-{target}.tar.gz"),
            sha256: "a".repeat(64),
            size: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    const TARGET: &str = "x86_64-unknown-linux-musl";

    fn info(tag: &str, prerelease: bool) -> ReleaseInfo {
        ReleaseInfo {
            tag: tag.into(),
            draft: false,
            prerelease,
            published: true,
        }
    }

    fn v(text: &str) -> semver::Version {
        semver::Version::parse(text).unwrap()
    }

    #[test]
    fn the_highest_semver_wins_whatever_the_host_order() {
        let host = FakeHost {
            releases: vec![
                info("v0.3.10", false),
                info("v0.10.0", false),
                info("v0.9.0", false),
                info("v1.0.0-beta.1", true),
                info("not-a-tag", false),
                ReleaseInfo {
                    tag: "v9.9.9".into(),
                    draft: true,
                    prerelease: false,
                    published: false,
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            newest(&host, ReleaseChannel::Stable).unwrap().unwrap().0,
            v("0.10.0")
        );
        assert_eq!(
            newest(&host, ReleaseChannel::Prerelease)
                .unwrap()
                .unwrap()
                .0,
            v("1.0.0-beta.1")
        );
    }

    #[test]
    fn the_scan_continues_across_pages_and_fails_rather_than_guess_at_its_bound() {
        let mut releases: Vec<_> = (0..150)
            .map(|i| info(&format!("v0.1.{i}"), false))
            .collect();
        releases.push(info("v0.2.0", false));
        let host = FakeHost {
            releases,
            ..Default::default()
        };
        assert_eq!(
            newest(&host, ReleaseChannel::Stable).unwrap().unwrap().0,
            v("0.2.0")
        );

        let full: Vec<_> = (0..MAX_RELEASES_SCANNED)
            .map(|i| info(&format!("v0.0.{i}"), false))
            .collect();
        let host = FakeHost {
            releases: full,
            ..Default::default()
        };
        assert_eq!(
            crate::installation::failure_of(&newest(&host, ReleaseChannel::Stable).unwrap_err()),
            Some(InstallationFailure::ReleaseEnumerationLimitExceeded)
        );
        let mut exact_fit: Vec<_> = (0..MAX_RELEASES_SCANNED - 1)
            .map(|i| info(&format!("v0.0.{i}"), false))
            .collect();
        exact_fit.push(info("v5.0.0", false));
        let host = FakeHost {
            releases: exact_fit,
            ..Default::default()
        };
        assert!(
            newest(&host, ReleaseChannel::Stable).is_err(),
            "a full final page is not exhaustion"
        );
    }

    #[test]
    fn exact_bytes_are_verified_before_parsing() {
        let key = Key::new(1);
        let mut host = FakeHost::default();
        host.publish("1.0.0", false, &key, &[], &[&key], vec![artifact(TARGET)]);
        let verified = verify_release(&host, "v1.0.0", &trust(&[&key])).unwrap();
        assert_eq!(verified.key_id, key.id);
        let manifest = host
            .assets
            .get_mut(&("v1.0.0".into(), MANIFEST_ASSET.into()))
            .unwrap();
        manifest.push(b' ');
        assert_eq!(
            crate::installation::failure_of(
                &verify_release(&host, "v1.0.0", &trust(&[&key])).unwrap_err()
            ),
            Some(InstallationFailure::ReleaseSignatureInvalid)
        );
    }

    #[test]
    fn an_old_client_finds_the_overlap_signature_for_the_key_it_knows() {
        let (old, new) = (Key::new(1), Key::new(2));
        let mut host = FakeHost::default();
        host.publish(
            "2.0.0",
            false,
            &new,
            &[&old, &new],
            &[&old, &new],
            vec![artifact(TARGET)],
        );
        let verified = verify_release(&host, "v2.0.0", &trust(&[&old])).unwrap();
        assert_eq!(verified.key_id, old.id);
        assert!(host.fetches.borrow().len() <= 1 + 1 + 1);
        // A per-key file whose envelope names another key is rejected.
        let bytes = host.assets[&("v2.0.0".to_string(), MANIFEST_ASSET.to_string())].clone();
        host.assets.insert(
            ("v2.0.0".into(), format!("release-manifest.{}.sig", old.id)),
            new.envelope(&bytes),
        );
        assert!(verify_release(&host, "v2.0.0", &trust(&[&old])).is_err());
        // Nothing trusted at all fails closed.
        let stranger = Key::new(9);
        assert!(verify_release(&host, "v2.0.0", &trust(&[&stranger])).is_err());
    }

    #[test]
    fn the_signature_request_count_is_bounded_by_the_key_set() {
        let keys: Vec<Key> = (1..=4).map(Key::new).collect();
        let signer = Key::new(7);
        let mut host = FakeHost::default();
        host.publish(
            "1.0.0",
            false,
            &signer,
            &[],
            &[&signer],
            vec![artifact(TARGET)],
        );
        let refs: Vec<&Key> = keys.iter().collect();
        assert!(verify_release(&host, "v1.0.0", &trust(&refs)).is_err());
        let signature_requests = host
            .fetches
            .borrow()
            .iter()
            .filter(|asset| asset.ends_with(".sig"))
            .count();
        assert_eq!(signature_requests, 1 + keys.len());
        assert!(signature_requests <= 1 + MAX_TRUSTED_RELEASE_KEYS);
    }

    #[test]
    fn the_embedded_key_set_is_guarded() {
        let (a, b) = (Key::new(1), Key::new(2));
        assert!(TrustSet::new(&[(a.id.as_str(), a.public.as_str())]).is_ok());
        assert!(
            TrustSet::new(&[(b.id.as_str(), a.public.as_str())]).is_err(),
            "declared != derived"
        );
        assert!(TrustSet::new(&[
            (a.id.as_str(), a.public.as_str()),
            (a.id.as_str(), a.public.as_str())
        ])
        .is_err());
        let five: Vec<Key> = (1..=5).map(Key::new).collect();
        let entries: Vec<(&str, &str)> = five
            .iter()
            .map(|k| (k.id.as_str(), k.public.as_str()))
            .collect();
        assert!(TrustSet::new(&entries).is_err());
        // The production set passes its own guard and never contains a test key.
        let embedded = TrustSet::embedded().unwrap();
        assert!(!embedded.ids().contains(&a.id));
    }

    #[test]
    fn a_dormant_client_updates_through_a_stable_bridge() {
        let (a, b) = (Key::new(1), Key::new(2));
        let mut host = FakeHost::default();
        host.publish("1.1.0", false, &a, &[], &[&a, &b], vec![artifact(TARGET)]);
        host.publish("2.0.0", false, &b, &[], &[&b], vec![artifact(TARGET)]);
        let installed = v("1.0.0");
        for channel in [ReleaseChannel::Stable, ReleaseChannel::Prerelease] {
            match plan(&host, &trust(&[&a]), &installed, channel, None, TARGET, 0).unwrap() {
                Plan::Bridge { bridge, target } => {
                    assert_eq!(bridge.version, v("1.1.0"));
                    assert_eq!(target, v("2.0.0"));
                }
                other => panic!("{other:?}"),
            }
        }
        // After the hop the new binary trusts both and installs directly.
        assert!(matches!(
            plan(
                &host,
                &trust(&[&a, &b]),
                &v("1.1.0"),
                ReleaseChannel::Stable,
                None,
                TARGET,
                1
            )
            .unwrap(),
            Plan::Install(_)
        ));
        assert_eq!(
            crate::installation::failure_of(
                &plan(
                    &host,
                    &trust(&[&a]),
                    &installed,
                    ReleaseChannel::Stable,
                    None,
                    TARGET,
                    2
                )
                .unwrap_err()
            ),
            Some(InstallationFailure::ReleaseTrustHopLimitExceeded)
        );
    }

    #[test]
    fn a_prerelease_only_bridge_never_serves_a_stable_client() {
        let (a, b) = (Key::new(1), Key::new(2));
        let mut host = FakeHost::default();
        host.publish(
            "2.0.0-beta.1",
            true,
            &a,
            &[],
            &[&a, &b],
            vec![artifact(TARGET)],
        );
        host.publish("2.0.0", false, &b, &[], &[&b], vec![artifact(TARGET)]);
        assert_eq!(
            crate::installation::failure_of(
                &plan(
                    &host,
                    &trust(&[&a]),
                    &v("1.0.0"),
                    ReleaseChannel::Stable,
                    None,
                    TARGET,
                    0
                )
                .unwrap_err()
            ),
            Some(InstallationFailure::ReleaseTrustBridgeUnavailable)
        );
        let published = vec![PublishedRelease {
            version: v("2.0.0-beta.1"),
            stable: false,
            signed_by: vec![a.id.clone()],
            trusted_key_ids: vec![a.id.clone(), b.id.clone()],
        }];
        assert!(check_retirement_bridge(&published, &a.id, &b.id).is_err());
        let mut with_stable = published;
        with_stable.push(PublishedRelease {
            version: v("1.1.0"),
            stable: true,
            signed_by: vec![a.id.clone()],
            trusted_key_ids: vec![a.id.clone(), b.id.clone()],
        });
        check_retirement_bridge(&with_stable, &a.id, &b.id).unwrap();
    }

    #[test]
    fn a_prerelease_track_may_consume_a_stable_classified_manifest() {
        let key = Key::new(1);
        let mut host = FakeHost::default();
        host.publish(
            "1.0.0-rc.1",
            true,
            &key,
            &[],
            &[&key],
            vec![artifact(TARGET)],
        );
        host.publish("1.0.0", false, &key, &[], &[&key], vec![artifact(TARGET)]);
        match plan(
            &host,
            &trust(&[&key]),
            &v("0.9.0"),
            ReleaseChannel::Prerelease,
            None,
            TARGET,
            0,
        )
        .unwrap()
        {
            Plan::Install(release) => assert_eq!(release.manifest.channel, "stable"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn release_time_trust_sets_must_match_exactly_and_in_order() {
        let ids = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let target = |list: &[&str]| ("t".to_string(), ids(list));
        check_trust_set_equality(&ids(&["A", "B"]), &[target(&["A", "B"])]).unwrap();
        assert!(check_trust_set_equality(&ids(&["A", "B"]), &[target(&["A"])]).is_err());
        assert!(check_trust_set_equality(&ids(&["A"]), &[target(&["A", "B"])]).is_err());
        assert!(check_trust_set_equality(&ids(&["A", "B"]), &[target(&["B", "A"])]).is_err());
        assert!(check_trust_set_equality(&ids(&["A", "A"]), &[target(&["A", "A"])]).is_err());
        assert!(check_trust_set_equality(
            &ids(&["A", "B"]),
            &[target(&["A", "B"]), target(&["B"])]
        )
        .is_err());
    }
}
