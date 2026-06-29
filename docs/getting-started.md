# Getting started

## Setup
### Build
Build from source (Rust 1.75+):
```
git clone https://github.com/Shiva936/draft.git
cd <project-directory>/draft
cargo build --release
# binaries: target/release/draft and target/release/draftd
```

### Install
Install the CLI and optional daemon into ~/.cargo/bin :
```
cargo install --path cli --locked
cargo install --path services/draftd --locked
```

### Verify
Confirm your shell can find both binaries:
```
draft --version
draft service status
# expected CLI version is draft x.x.x(release version).
```

## First workflow (Git)
```
cd my-git-repo
draft workspace detect          # confirms the Git provider
draft workspace init            # creates .draft/ and excludes it from history
# ... edit files ...
draft status                    # provider-neutral change summary
draft review --yes              # approve the change groups
draft verify "cargo test"       # run a verification command (shown before it runs)
draft commit -m "my change"     # finalize into a Git commit + write a receipt
draft receipt list              # see the receipt mapping change → commit
draft undo                      # safely reverse the last finalization
```

## Optional daemon
```
draft service start    # start draftd and register this workspace if initialized
draft service status
draft service stop
```

## Manage per-project policy and verification

Optionally edit per-project policy under `.draft/config.toml`.

Recommended stricter defaults for real projects:
```
[finalization]
require_review = true
require_verification = true
block_on_high_risk = true
allow_unverified = false
allow_high_risk_with_confirmation = true
```

Add project-specific verification commands under `.draft/config.toml`.

Rust project:
```
[[verification.commands]]
name = "test"
command = "cargo"
args = ["test"]
timeout_ms = 600000
```

Node project:
```
[[verification.commands]]
name = "test"
command = "npm"
args = ["test"]
timeout_ms = 600000
```

## Suggestions
Use Draft during normal work:
```
draft status
draft review
draft verify
draft commit -m "Your commit message"
draft receipt list
```

Use the daemon only if you want long-running local coordination:
```
draft service start
draft service status
draft service stop
```
Draft still works in embedded CLI mode when draftd is not running.

To use an experimental provider, opt in explicitly:
```
draft workspace init --provider jj --experimental
```
For real projects, prefer Git until those providers are completed beyond detection/capability scaffolding.

## Release smoke environments
The release-readiness workflow runs format, clippy, tests, doctests, and release builds on Linux, macOS, and Windows. For WSL, run the same smoke locally inside the target WSL distribution:

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo test --workspace --doc
cargo build --workspace --release
```
