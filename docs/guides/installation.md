# Installation

Draft publishes versioned GitHub Release binaries for Linux, macOS, WSL, and native Windows. Building from source is also supported.

## Requirements

- A local shell appropriate for your platform.
- A workspace directory where Draft can create `.draft/`.

Rust is only required when building from source.

## Install Latest Release

Linux, macOS, or WSL:

```bash
curl -fsSL https://raw.githubusercontent.com/Shiva936/draft/master/install.sh | sh
```

Native Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/Shiva936/draft/master/install.ps1 | iex
```

The installers resolve the latest GitHub Release from `Shiva936/draft` (or the exact release named by `DRAFT_VERSION` / `-Version`), download the matching archive for the current operating system and CPU, verify it against the release's published `SHA256SUMS` over HTTPS, and then hand the verified package to Draft's own installer coordinator, which performs the installation. The shell and PowerShell scripts never write the installation themselves.

**Installer trust.** The first install trusts HTTPS and `SHA256SUMS`. It does not check the signed release manifest — there is no trusted Draft binary yet to check it with. Once Draft is installed, every `draft update` verifies the signed release manifest before it replaces anything.

## Supported Release Targets

| Platform            | Target                       |
| ------------------- | ---------------------------- |
| Linux / WSL x86_64  | `x86_64-unknown-linux-musl`  |
| Linux / WSL arm64   | `aarch64-unknown-linux-musl` |
| macOS Intel         | `x86_64-apple-darwin`        |
| macOS Apple Silicon | `aarch64-apple-darwin`       |
| Windows x86_64      | `x86_64-pc-windows-msvc`     |

Windows arm64 is not a v0.3.4 binary target. Unsupported systems fail before download.

## Install Location and PATH

Draft installs into a dedicated installation root and exposes it on PATH:

| Platform | Installation root | On PATH as |
| --- | --- | --- |
| Linux, macOS, WSL | `$HOME/.local/share/draft` | symlinks `$HOME/.local/bin/draft` and `$HOME/.local/bin/draftd` pointing into `<root>/bin` |
| Windows | `%LOCALAPPDATA%\Programs\Draft` | `<root>\bin` on your User PATH, only if you ask |

```text
<install_root>/
├── bin/draft, bin/draftd
└── .draft-install/        Draft's private installation metadata
```

On Unix the PATH entries are **symlinks, never copies**: a copied `draft` is not a managed installation, and `draft update` / `draft uninstall` will refuse to manage it. If a symlink cannot be created the install fails rather than copying.

Set `DRAFT_INSTALL_ROOT` to choose another installation root. On Unix, `DRAFT_INSTALL_DIR` keeps its meaning — the PATH directory the symlinks go in (default `$HOME/.local/bin`):

```bash
curl -fsSL https://raw.githubusercontent.com/Shiva936/draft/master/install.sh | DRAFT_INSTALL_ROOT="$HOME/opt/draft" DRAFT_INSTALL_DIR="$HOME/bin" sh
```

On Windows, `-InstallRoot` / `$env:DRAFT_INSTALL_ROOT` selects the root. `DRAFT_INSTALL_DIR` alone still works and names the `bin` directory, so the root is its parent; if you set both, `DRAFT_INSTALL_DIR` must be `<DRAFT_INSTALL_ROOT>\bin` or the installer refuses.

Installers do not silently edit PATH. On Unix, when the PATH directory is missing from PATH they print exact instructions; `DRAFT_UPDATE_PATH=1` lets the installer add it to your shell profile. On Windows, `-UpdatePath` or `DRAFT_UPDATE_PATH=1` adds `<root>\bin` to your User PATH — only if it is not already reachable — and Draft remembers that it added it:

```bash
curl -fsSL https://raw.githubusercontent.com/Shiva936/draft/master/install.sh | DRAFT_UPDATE_PATH=1 sh
```

```powershell
$env:DRAFT_UPDATE_PATH = "1"; irm https://raw.githubusercontent.com/Shiva936/draft/master/install.ps1 | iex
```

### Moving from a copied install

Earlier installers copied `draft` and `draftd` straight into `$HOME/.local/bin`. The installer now **refuses** to replace those regular files by default, because it cannot know an unrelated program named `draft` is not yours. Either remove or rename both copied files, or set `DRAFT_MIGRATE_LEGACY_PATH=1` to authorize replacing them (both must be Draft and report the same version); then run the installer, which installs into the dedicated root and creates the symlinks. On Windows, re-running `install.ps1` is enough.

### WSL Runtime Directory Troubleshooting

`draftd` is a user-scoped process and should not be run with `sudo`. If WSL sets `XDG_RUNTIME_DIR` to a missing or unusable directory such as `/run/user/1000`, Draft falls back to `~/.local/state/draft/draftd.sock`.

For releases without the automatic fallback, recover the current shell with:

```bash
unset XDG_RUNTIME_DIR
draftd --detach
draftd status
```

`draftd start` runs in the foreground. Use `draftd --detach` or `draft daemon start` when the daemon should continue in the background.

## Update

```bash
draft update --check      # what is available; changes nothing
draft update              # newest release on your channel
draft update --channel prerelease
draft update --version 0.3.5
```

Only official installations update themselves. Package-manager and source builds are reported, with the command to use instead, and are never modified.

- **Channels.** `stable` (the default) considers only stable releases; `prerelease` considers prereleases _and_ stable releases, so it never leaves you behind stable. The target is the highest eligible version, not the most recently published one. `--channel` switches your track; if the newest release on the new track is the one you already have, only the recorded channel changes.
- **Exact versions.** `--version` installs exactly that release, whatever your channel, and never changes the channel. Installing an older version needs `--allow-downgrade`.
- **Verification.** Every artifact is checked against a signed release manifest before it replaces anything.
- **The daemon.** If `draftd` was running it is stopped for the swap and restarted afterwards; if it was not running it stays stopped.
- **Interruptions.** Both binaries are replaced as one recoverable transaction. If an update is interrupted, the next `draft update` or `draft uninstall` finishes it or rolls it back before doing anything else.

**Release signing keys rotate.** A Draft that has been offline through a key rotation may update through one or more intermediate _trust-bridge_ releases to refresh what it trusts; `draft update` says so as it happens, and `--check` reports it. If no bridge can reach your installation, Draft reports that it is below the supported self-update trust floor: reinstall with the official installer.

## Uninstall

```bash
draft uninstall --dry-run   # exactly what would be removed and kept
draft uninstall
```

`draft uninstall` removes both executables, their PATH exposure (the two Unix symlinks, or on Windows only the User PATH entry this installation added — never an entry that was already there), and the installation's own metadata. It keeps:

- every project;
- the global user store at `$DRAFT_GLOBAL_HOME` or `$HOME/.draft/` (registry, identity keys, trust, extensions and settings);
- a small inert lock file, `<root>/.draft-install/lifecycle.lock`, which a later install reuses.

> `draft uninstall` does not remove Draft metadata from your projects. Your `.draft/` directories are never scanned and never deleted.

**Purge.** `draft uninstall --purge` also deletes the global user store — including your identity keys and the project registry — after confirmation (`--yes` confirms non-interactively). It does so only after proving the directory is a Draft-managed store: stores Draft created carry a `home.json` ownership marker. A custom `DRAFT_GLOBAL_HOME`, or a store that predates the marker, is refused for purge; remove it yourself if it is yours. `--yes` skips only the prompt, never these checks.

**Interrupted uninstalls.** Uninstall is transactional and resumable, and an interrupted one is completed rather than half-left. Draft stages a copy of itself inside the installation's private directory before removing anything, and that copy finishes the job. If your machine restarted after the binaries were already gone, re-run the official installer: it finds the unfinished uninstall and completes it before installing anything. For a custom root, re-run it with the same `DRAFT_INSTALL_ROOT`; the installer checks exactly that root and never searches your disk. Please do not delete Draft's lifecycle files by hand while an operation is active.

**On Windows.** Uninstall never requires administrator rights and never requires a reboot. Draft changes only its own User PATH entry and never rewrites other entries it read. If another program changes PATH at the same moment and Draft cannot confirm the result, Draft stops before removing anything else, and the uninstall can simply be run again; Draft does not claim to coordinate with programs that do not use its lock. Draft may briefly leave a tiny private lifecycle residue — containing no Draft executable and no project data — which the next official installer clears or reuses safely.

## Build From Source

Source builds require a stable Rust toolchain.

```bash
cargo build --workspace
```

Run the CLI during development:

```bash
cargo run -p draft-cli -- --help
cargo run -p draft-cli -- init
```

After installing or copying the built binary into your `PATH`, use:

```bash
draft --help
draft init
```

## Verify The Build

```bash
cargo fmt --check
cargo test
```

## Notes

Draft does not require a hosted service for core CLI workflows. Optional local service crates exist for background and live flows, but the CLI calls core behavior directly.
