# Installation

Draft v0.3.1 is a Rust workspace. The current public path is building the CLI from source.

## Requirements

- A stable Rust toolchain.
- A local shell appropriate for your platform.
- A workspace directory where Draft can create `.draft/`.

## Build From Source

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

