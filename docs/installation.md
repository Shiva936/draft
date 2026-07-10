# Installation

Draft v0.3.3 publishes versioned GitHub Release binaries for Linux, macOS, WSL, and native Windows. Building from source is also supported.

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

The installers resolve the latest GitHub Release from `Shiva936/draft`, download the matching archive for the current operating system and CPU, verify it against `SHA256SUMS`, and install both `draft` and the optional `draftd` local service binary.

## Supported Release Targets

| Platform | Target |
| --- | --- |
| Linux / WSL x86_64 | `x86_64-unknown-linux-musl` |
| Linux / WSL arm64 | `aarch64-unknown-linux-musl` |
| macOS Intel | `x86_64-apple-darwin` |
| macOS Apple Silicon | `aarch64-apple-darwin` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |

Windows arm64 is not a v0.3.3 binary target. Unsupported systems fail before download.

## Install Location and PATH

On Linux, macOS, and WSL, the default install directory is:

```text
$HOME/.local/bin
```

Set `DRAFT_INSTALL_DIR` to choose another directory:

```bash
curl -fsSL https://raw.githubusercontent.com/Shiva936/draft/master/install.sh | DRAFT_INSTALL_DIR="$HOME/bin" sh
```

On Windows, the default install directory is:

```text
%LOCALAPPDATA%\Programs\Draft\bin
```

Set `$env:DRAFT_INSTALL_DIR` before running the installer to choose another directory.

Installers do not silently edit PATH. If the install directory is missing from PATH, they print exact instructions. To allow PATH updates:

```bash
curl -fsSL https://raw.githubusercontent.com/Shiva936/draft/master/install.sh | DRAFT_UPDATE_PATH=1 sh
```

```powershell
$env:DRAFT_UPDATE_PATH = "1"; irm https://raw.githubusercontent.com/Shiva936/draft/master/install.ps1 | iex
```

## Upgrades

Re-run the installer to upgrade. Existing `draft` and `draftd` binaries in the install directory are replaced after the release archive passes checksum verification.

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
