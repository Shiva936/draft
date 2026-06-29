# Getting started

## Install
Build from source (Rust 1.75+):

```
cargo build --release
# binaries: target/release/draft and target/release/draftd
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

## Release smoke environments
The release-readiness workflow runs format, clippy, tests, doctests, and release builds on Linux, macOS, and Windows. For WSL, run the same smoke locally inside the target WSL distribution:

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo test --workspace --doc
cargo build --workspace --release
```
