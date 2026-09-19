//! `draft update`, `draft uninstall`, and the three hidden lifecycle surfaces
//! (the lifecycle helper, `release-trust-set --json`, and the installer
//! coordinator / generic recovery entry point).
//!
//! These manage the Draft *installation*, never a project: they run before
//! project discovery, open no project, and exist only on the CLI — no IPC
//! method, Console route or TUI entry reaches them.

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use draft_core::installation::{
    self as lifecycle, coordinator, layout, operation, provenance, receipt, release, uninstall,
    update, InstallPlatform, InstallationFailure, InstallationId, InstallationOperationId,
    LifecycleHost, NoFaults,
};
use draft_core::support::error::DraftError;

use crate::{output, service};

/// The repository the updater resolves releases from. Configured here, never
/// taken from user input.
const RELEASE_REPOSITORY: &str = "Shiva936/draft";
/// Hosts a release request may be served from or redirected to.
const RELEASE_HOSTS: &[&str] = &[
    "api.github.com",
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
const DAEMON_TIMEOUT: Duration = Duration::from_secs(5);

/// The real machine.
pub struct SystemHost;

impl LifecycleHost for SystemHost {
    fn daemon_running(&self) -> bool {
        service::daemon_running()
    }

    fn stop_daemon(&self) -> Result<(), DraftError> {
        service::stop_and_wait(DAEMON_TIMEOUT)
    }

    fn start_daemon(&self, draftd: &Path) -> Result<(), DraftError> {
        service::start_at_and_wait(draftd, DAEMON_TIMEOUT)
    }

    fn daemon_healthy(&self) -> bool {
        service::status_ok()
    }

    fn binary_version(&self, exe: &Path) -> Result<String, DraftError> {
        let mut child = std::process::Command::new(exe)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(DraftError::from)?;
        let deadline = Instant::now() + lifecycle::BINARY_VALIDATION_TIMEOUT;
        loop {
            if child.try_wait().map_err(DraftError::from)?.is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                return Err(lifecycle::fail(
                    InstallationFailure::ValidationFailed,
                    format!("{} --version timed out", exe.display()),
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut text = String::new();
        if let Some(mut stdout) = child.stdout.take() {
            stdout.read_to_string(&mut text).map_err(DraftError::from)?;
        }
        lifecycle::parse_reported_version(&text).ok_or_else(|| {
            lifecycle::fail(
                InstallationFailure::ValidationFailed,
                format!("{} reported no version", exe.display()),
            )
        })
    }

    fn launch_helper(
        &self,
        helper: &Path,
        installation: &InstallationId,
        operation: &InstallationOperationId,
    ) -> Result<(), DraftError> {
        let mut command = std::process::Command::new(helper);
        command
            .args([
                "__lifecycle-helper",
                "--installation-id",
                installation.as_str(),
            ])
            .args(["--operation-id", operation.as_str()])
            .args(["--parent-pid", &std::process::id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            command.creation_flags(DETACHED_PROCESS);
        }
        command.spawn().map(|_| ()).map_err(DraftError::from)
    }

    fn wait_for_exit(&self, pid: u32, timeout: Duration) -> Result<(), DraftError> {
        lifecycle::wait_for_process_exit(pid, timeout)
    }
}

/// Releases over HTTPS from the configured repository: pinned hosts, bounded
/// redirects, 5 s connect timeout and bounded reads.
pub struct GitHubReleases {
    metadata: reqwest::blocking::Client,
    downloads: reqwest::blocking::Client,
}

fn network(error: impl std::fmt::Display) -> DraftError {
    lifecycle::fail(InstallationFailure::NetworkFailure, error.to_string())
}

impl GitHubReleases {
    pub fn new() -> Result<Self, DraftError> {
        let client = |timeout: Duration| {
            reqwest::blocking::Client::builder()
                .user_agent(concat!("draft-updater/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(Duration::from_secs(5))
                .timeout(timeout)
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    let allowed = attempt.url().scheme() == "https"
                        && attempt
                            .url()
                            .host_str()
                            .is_some_and(|host| RELEASE_HOSTS.contains(&host));
                    if attempt.previous().len() < 5 && allowed {
                        attempt.follow()
                    } else {
                        attempt.stop()
                    }
                }))
                .build()
                .map_err(network)
        };
        Ok(Self {
            metadata: client(Duration::from_secs(20))?,
            // An artifact is tens of megabytes; the metadata bound would fail it
            // on an ordinary connection. It stays capped by size either way.
            downloads: client(Duration::from_secs(600))?,
        })
    }

    fn get(
        &self,
        client: &reqwest::blocking::Client,
        url: &str,
    ) -> Result<Option<reqwest::blocking::Response>, DraftError> {
        let response = client
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .map_err(network)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(network(format!("{url}: HTTP {}", response.status())));
        }
        Ok(Some(response))
    }

    fn info(value: &serde_json::Value) -> Option<release::ReleaseInfo> {
        Some(release::ReleaseInfo {
            tag: value.get("tag_name")?.as_str()?.to_string(),
            draft: value.get("draft")?.as_bool()?,
            prerelease: value.get("prerelease")?.as_bool()?,
            published: value.get("published_at").is_some_and(|at| !at.is_null()),
        })
    }

    fn asset_url(tag: &str, asset: &str) -> String {
        format!("https://github.com/{RELEASE_REPOSITORY}/releases/download/{tag}/{asset}")
    }
}

impl release::ReleaseSource for GitHubReleases {
    fn list(&self, page: usize, per_page: usize) -> Result<Vec<release::ReleaseInfo>, DraftError> {
        let url = format!(
            "https://api.github.com/repos/{RELEASE_REPOSITORY}/releases?per_page={per_page}&page={page}"
        );
        let Some(response) = self.get(&self.metadata, &url)? else {
            return Ok(Vec::new());
        };
        let body: serde_json::Value = serde_json::from_reader(response).map_err(network)?;
        let entries = body.as_array().ok_or_else(|| {
            lifecycle::fail(
                InstallationFailure::ReleaseMetadataInvalid,
                "the release list is not an array",
            )
        })?;
        entries
            .iter()
            .map(|entry| {
                Self::info(entry).ok_or_else(|| {
                    lifecycle::fail(
                        InstallationFailure::ReleaseMetadataInvalid,
                        "a release entry is malformed",
                    )
                })
            })
            .collect()
    }

    fn by_tag(&self, tag: &str) -> Result<Option<release::ReleaseInfo>, DraftError> {
        let url = format!("https://api.github.com/repos/{RELEASE_REPOSITORY}/releases/tags/{tag}");
        let Some(response) = self.get(&self.metadata, &url)? else {
            return Ok(None);
        };
        let body: serde_json::Value = serde_json::from_reader(response).map_err(network)?;
        Ok(Self::info(&body))
    }

    fn fetch(&self, tag: &str, asset: &str, cap: u64) -> Result<Option<Vec<u8>>, DraftError> {
        let Some(response) = self.get(&self.metadata, &Self::asset_url(tag, asset))? else {
            return Ok(None);
        };
        let too_large = || {
            let kind = if asset == release::MANIFEST_ASSET {
                InstallationFailure::ReleaseManifestTooLarge
            } else {
                InstallationFailure::ReleaseMetadataInvalid
            };
            lifecycle::fail(kind, format!("{asset} exceeds {cap} bytes"))
        };
        if response.content_length().is_some_and(|length| length > cap) {
            return Err(too_large());
        }
        let mut bytes = Vec::new();
        response
            .take(cap + 1)
            .read_to_end(&mut bytes)
            .map_err(network)?;
        if bytes.len() as u64 > cap {
            return Err(too_large());
        }
        Ok(Some(bytes))
    }

    fn fetch_to(&self, tag: &str, asset: &str, cap: u64, dest: &Path) -> Result<bool, DraftError> {
        let Some(mut response) = self.get(&self.downloads, &Self::asset_url(tag, asset))? else {
            return Ok(false);
        };
        let too_large = || {
            lifecycle::fail(
                InstallationFailure::ReleaseArtifactTooLarge,
                format!("{asset} exceeds {cap} bytes"),
            )
        };
        if response.content_length().is_some_and(|length| length > cap) {
            return Err(too_large());
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dest)
            .map_err(DraftError::from)?;
        let mut buffer = vec![0u8; 64 * 1024];
        let mut written = 0u64;
        loop {
            let read = response.read(&mut buffer).map_err(network)?;
            if read == 0 {
                break;
            }
            written += read as u64;
            if written > cap {
                return Err(too_large());
            }
            file.write_all(&buffer[..read]).map_err(DraftError::from)?;
        }
        file.sync_all().map_err(DraftError::from)?;
        Ok(true)
    }
}

fn current_exe() -> Result<PathBuf, DraftError> {
    std::env::current_exe().map_err(DraftError::from)
}

fn platform() -> InstallPlatform {
    InstallPlatform::current()
}

#[cfg(windows)]
fn system_registry() -> Option<&'static dyn lifecycle::path::windows::UserPathRegistry> {
    static REGISTRY: lifecycle::path::windows::SystemRegistry =
        lifecycle::path::windows::SystemRegistry;
    Some(&REGISTRY)
}

#[cfg(not(windows))]
fn system_registry() -> Option<&'static dyn lifecycle::path::windows::UserPathRegistry> {
    None
}

/// Resolve any interrupted operation on this installation before starting a
/// new one: generic recovery, dispatching by kind.
fn recover_first(own: &Path) -> Result<(), DraftError> {
    let host = SystemHost;
    let ctx = coordinator::Context {
        host: &host,
        registry: system_registry(),
        faults: &NoFaults,
        platform: platform(),
    };
    coordinator::recover_installed(&ctx, own, |layout, op| {
        let source = GitHubReleases::new()?;
        let trust = release::TrustSet::embedded()?;
        let target = lifecycle::current_target()?;
        update::recover(
            &update::Context {
                layout,
                host: &host,
                source: &source,
                trust: &trust,
                platform_target: target,
                faults: &NoFaults,
            },
            op,
        )
    })
}

#[allow(clippy::too_many_arguments)]
pub fn run_update(
    check: bool,
    version: Option<String>,
    channel: Option<String>,
    allow_downgrade: bool,
    hops: u32,
    json: bool,
) -> Result<(), DraftError> {
    let request = update::UpdateRequest {
        check,
        version: version
            .map(|value| {
                semver::Version::parse(value.trim_start_matches('v')).map_err(|_| {
                    lifecycle::fail(
                        InstallationFailure::ReleaseUnavailable,
                        format!("'{value}' is not a SemVer version"),
                    )
                })
            })
            .transpose()?,
        channel: channel
            .as_deref()
            .map(receipt::ReleaseChannel::parse)
            .transpose()?,
        allow_downgrade,
        hops,
    };
    // Flags are judged before any network call.
    request.validate()?;
    let own = current_exe()?;
    let (layout, _) =
        provenance::require_official(provenance::detect(&own, platform()), "draft update")?;
    if operation::read(&layout)?.is_some() {
        recover_first(&own)?;
    }
    let host = SystemHost;
    let source = GitHubReleases::new()?;
    let trust = release::TrustSet::embedded()?;
    let target = lifecycle::current_target()?;
    let ctx = update::Context {
        layout: &layout,
        host: &host,
        source: &source,
        trust: &trust,
        platform_target: target,
        faults: &NoFaults,
    };
    let outcome = update::run(&ctx, &request)?;
    if let update::UpdateOutcome::Bridged { via, target } = &outcome {
        if !json {
            output::line(&format!(
                "Updating through {via} to refresh release-signing trust, then to {target}."
            ));
        }
        // Continue from the newly installed binary, which now embeds the
        // expanded trust set. The hop counter bounds the chain.
        let mut next = std::process::Command::new(layout.executable(layout::Executable::Draft));
        next.arg("update")
            .args(["--trust-hop", &(hops + 1).to_string()]);
        if let Some(channel) = &request.channel {
            next.args(["--channel", channel.as_str()]);
        }
        if json {
            next.arg("--json");
        }
        let status = next.status().map_err(DraftError::from)?;
        if !status.success() {
            return Err(lifecycle::fail(
                InstallationFailure::RecoveryFailed,
                format!("the update continued from Draft {via} and failed"),
            ));
        }
        return Ok(());
    }
    if json {
        output::print_json(&outcome);
        return Ok(());
    }
    match outcome {
        update::UpdateOutcome::UpToDate { version, .. } => {
            output::success(&format!("Draft {version} is already up to date."))
        }
        update::UpdateOutcome::ChannelChanged { version, from, to } => output::success(&format!(
            "Draft {version} now follows the {} channel (was {}).",
            to.as_str(),
            from.as_str()
        )),
        update::UpdateOutcome::Updated { from, to, .. } => {
            output::success(&format!("Updated Draft {from} → {to}."))
        }
        update::UpdateOutcome::Checked(report) => {
            output::header("Draft update");
            output::field("Current", &report.current);
            output::field("Channel", report.channel.as_str());
            output::field("Latest", report.latest.as_deref().unwrap_or("unknown"));
            output::field(
                "Update available",
                if report.update_available { "yes" } else { "no" },
            );
            if let Some(bridge) = &report.bridge {
                output::field(
                    "Trust bridge",
                    &format!("{bridge} would be installed first"),
                );
            }
            if report.below_trust_floor {
                output::warn("This installation is below the self-update trust floor; reinstall with the official installer.");
            }
            if report.hop_limit_exceeded {
                output::warn("Reaching the latest release needs more trust-bridge updates than allowed; reinstall with the official installer.");
            }
        }
        update::UpdateOutcome::Bridged { .. } => unreachable!("handled above"),
    }
    Ok(())
}

fn confirm(prompt: &str) -> Result<bool, DraftError> {
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    print!("{prompt} Type 'yes' to continue: ");
    std::io::stdout().flush().map_err(DraftError::from)?;
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(DraftError::from)?;
    Ok(answer.trim() == "yes")
}

pub fn run_uninstall(dry_run: bool, purge: bool, yes: bool, json: bool) -> Result<(), DraftError> {
    let own = current_exe()?;
    let (layout, _) =
        provenance::require_official(provenance::detect(&own, platform()), "draft uninstall")?;
    if operation::read(&layout)?.is_some() {
        recover_first(&own)?;
        if operation::read(&layout)?.is_some() {
            output::success("An interrupted lifecycle operation was handed back to its executor; it finishes after this command exits.");
            return Ok(());
        }
    }
    let global_store = draft_core::project::home::DraftGlobalStore::locate()?;
    let host = SystemHost;
    let ctx = uninstall::Context {
        layout: &layout,
        host: &host,
        registry: system_registry(),
        faults: &NoFaults,
    };
    let _lock = operation::lock(&layout, lifecycle::LIFECYCLE_LOCK_TIMEOUT)?;
    let receipt = receipt::read_final(&layout)?;
    // Purge ownership is proven before the first durable write.
    let proven = if purge {
        Some(uninstall::prove_global_store(global_store.root(), &layout)?)
    } else {
        None
    };
    let plan = uninstall::plan(
        &layout,
        &receipt,
        system_registry(),
        proven.as_ref(),
        global_store.root(),
    )?;
    if dry_run {
        if json {
            output::print_json(&plan);
        } else {
            print_plan(&plan);
        }
        return Ok(());
    }
    if let Some(store) = &proven {
        if !yes
            && !confirm(&format!(
                "--purge deletes the Draft global store at {}, including identity keys and the project registry.",
                store.root.display()
            ))?
        {
            return Err(lifecycle::fail(
                InstallationFailure::GlobalStorePurgeUnsafe,
                "the purge was not confirmed; pass --yes to confirm non-interactively",
            ));
        }
    }
    let operation = uninstall::begin(&ctx, &receipt, proven)?;
    if json {
        output::print_json(
            &serde_json::json!({ "status": "handed_off", "operation_id": operation, "plan": plan }),
        );
    } else {
        print_plan(&plan);
        output::success("Uninstalling. The lifecycle helper finishes after this command exits.");
    }
    Ok(())
}

fn print_plan(plan: &uninstall::UninstallPlan) {
    output::header("Draft uninstall");
    output::field("Installation", &plan.install_root);
    output::section("Removes");
    for exe in &plan.remove_executables {
        output::bullet(exe);
    }
    match &plan.path {
        uninstall::PathPlan::Unix { remove_links } => {
            for link in remove_links {
                output::bullet(&format!("PATH link {link}"));
            }
        }
        uninstall::PathPlan::Windows { summary, .. } => output::bullet(summary),
    }
    for state in &plan.remove_lifecycle_state {
        output::bullet(state);
    }
    if let Some(store) = &plan.purge {
        output::bullet(&format!("the global user store at {store} (--purge)"));
    }
    output::section("Preserves");
    for kept in &plan.preserved {
        output::bullet(kept);
    }
    output::bullet(&format!("{} (the inert lifecycle lock)", plan.remains));
    output::line("");
    output::line("`draft uninstall` does not remove Draft metadata from your projects. Your .draft/ directories are never scanned and never deleted.");
}

/// Hidden: `release-trust-set --json`. Read-only; opens nothing.
pub fn run_release_trust_set(json: bool) -> Result<(), DraftError> {
    let ids: Vec<String> = release::RELEASE_TRUSTED_KEYS
        .iter()
        .map(|(id, _)| id.to_string())
        .collect();
    if json {
        output::print_json(&ids);
    } else {
        for id in ids {
            output::line(&id);
        }
    }
    Ok(())
}

/// Hidden: the lifecycle helper. Ids only — never a path.
pub fn run_helper(
    installation_id: String,
    operation_id: String,
    parent_pid: Option<u32>,
    bootstrap: bool,
) -> Result<(), DraftError> {
    if !InstallationId::is_well_formed(&installation_id)
        || !InstallationOperationId::is_well_formed(&operation_id)
    {
        return Err(lifecycle::fail(
            InstallationFailure::UninstallRecoveryFailed,
            "malformed helper invocation",
        ));
    }
    let mode = match (parent_pid, bootstrap) {
        (Some(pid), false) => uninstall::HelperMode::ParentExit { parent_pid: pid },
        (None, true) => uninstall::HelperMode::BootstrapRecovery,
        _ => {
            return Err(lifecycle::fail(
                InstallationFailure::UninstallRecoveryFailed,
                "malformed helper invocation",
            ))
        }
    };
    let operation = InstallationOperationId::new(operation_id);
    let installation = InstallationId::new(installation_id);
    let own = current_exe()?;
    let layout = uninstall::layout_of_helper(&own, &operation, platform())?;
    let host = SystemHost;
    let ctx = uninstall::Context {
        layout: &layout,
        host: &host,
        registry: system_registry(),
        faults: &NoFaults,
    };
    uninstall::run_helper(&ctx, &own, &installation, &operation, mode)
}

/// Hidden: the installer coordinator.
pub fn run_installer_install(
    install_root: PathBuf,
    path_bin: Option<PathBuf>,
    update_path: bool,
    migrate_legacy: bool,
    json: bool,
) -> Result<(), DraftError> {
    let package = own_package()?;
    let host = SystemHost;
    let ctx = coordinator::Context {
        host: &host,
        registry: system_registry(),
        faults: &NoFaults,
        platform: platform(),
    };
    let config = coordinator::InstallConfig {
        install_root,
        path_bin,
        update_path_requested: update_path,
        migrate_legacy,
        process_path: std::env::var("Path")
            .or_else(|_| std::env::var("PATH"))
            .ok(),
    };
    let outcome = coordinator::install(&ctx, &config, &package)?;
    if json {
        output::print_json(&outcome);
    } else {
        match outcome {
            coordinator::InstallOutcome::Installed {
                install_root,
                version,
                ..
            } => output::success(&format!("Installed Draft {version} into {install_root}.")),
            coordinator::InstallOutcome::AlreadyInstalled {
                install_root,
                version,
            } => output::success(&format!(
                "Draft {version} is already installed at {install_root}."
            )),
        }
    }
    Ok(())
}

/// Hidden: generic recovery (installed Draft, no root argument) or the
/// temporary coordinator's recovery-only mode (with the selected root).
pub fn run_installer_recover(install_root: Option<PathBuf>) -> Result<(), DraftError> {
    match install_root {
        None => recover_first(&current_exe()?),
        Some(root) => {
            let host = SystemHost;
            let ctx = coordinator::Context {
                host: &host,
                registry: system_registry(),
                faults: &NoFaults,
                platform: platform(),
            };
            coordinator::recover_as_coordinator(&ctx, &root, &own_package()?)
        }
    }
}

fn own_package() -> Result<coordinator::Package, DraftError> {
    let draft = layout::canonicalize(&current_exe()?)?;
    let draftd = draft
        .parent()
        .map(|bin| bin.join(format!("draftd{}", platform().exe_suffix())))
        .ok_or_else(|| {
            lifecycle::fail(InstallationFailure::ArchiveInvalid, "no package directory")
        })?;
    Ok(coordinator::Package {
        draft,
        draftd,
        version: draft_core::DRAFT_VERSION.to_string(),
        platform_target: lifecycle::current_target()?.to_string(),
    })
}
