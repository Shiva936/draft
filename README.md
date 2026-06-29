# Draft

Draft v0.3.0 is a local-first **Verified Changepacks + Review Cockpit** for human and AI-generated software changes.

Draft records work as reviewed, verified changepacks inside `.draft/`. The project does not model external history systems in v0.3.0. The only optional integration point is `target.local`, an opaque command string that runs after Draft save approval and safety checks.

## What Draft Is For

Draft is useful when a person or agent needs to produce a change that can be inspected before it becomes part of another workflow. It gives the workspace a native review layer:

- workspace scanning that includes files unknown to external tools;
- snapshots and checkpoints for reproducible baselines;
- changepacks with provenance hashes and evidence;
- verification, risk, policy, review, approval, save, receipts, and rollback;
- an append-only hash-chained event log;
- a CLI that works without a daemon and services that can power live flows.

Draft is intentionally local-first. It saves verified changepacks into `.draft/`. Any action outside Draft is explicit, local, and receipt-backed through `target.local`.

## Install From Source

```bash
cargo build --workspace
```

The CLI binary is `draft`. During development you can run it through Cargo:

```bash
cargo run -p draft-cli -- init
```

## Quick Start

```bash
draft init
draft config set identity.username "Ada"
draft config set identity.email "ada@example.com"

draft checkpoint "before change"

# Edit the workspace with your editor or agent.

draft status
draft pack create --name "update app" --from-working-tree
draft verify <pack-id>
draft risk <pack-id>
draft review <pack-id>
draft approve <pack-id> --reason "reviewed"
draft save <pack-id>
draft receipt list
```

The save step writes a durable receipt under `.draft/`. It does not require a daemon.

## Optional Local Save Command

`target.local` can run an opaque shell command after approval:

```bash
draft config set target.local "printf %s {message} > .last-draft-save"
```

Draft renders `{message}`, checks policy, verifies that `.draft/` is not part of the save candidate, executes the command from the workspace root, captures stdout, stderr, exit code, command hash, and writes all of that into the save receipt.

Draft does not parse, detect, or model what the command does.

## Core Commands

The main commands are `init`, `status`, `checkpoint`, `task`, `pack`, `spawn`, `runs`, `verify`, `risk`, `review`, `approve`, `reject`, `compare`, `compose`, `save`, `rollback`, `receipt`, `events`, `index`, and `service`.

See [docs/command-reference.md](docs/command-reference.md) for command behavior and JSON output notes.

## Storage And Safety

Draft stores its private state below `.draft/`: config and policy, content-addressed objects, snapshots and checkpoints, tasks, runs, changepacks, evidence, reviews, receipts, rebuildable indexes, and append-only hash-chained events.

`.draft/` is always hard-excluded from status, snapshots, changepacks, save candidates, rollback plans, and external command candidate checks. If a save candidate contains `.draft/`, Draft aborts the save, emits `SaveFailed`, records a failed receipt, and does not run `target.local`.

## Documentation

Start with [docs/README.md](docs/README.md). Important references:

- [Getting Started](docs/getting-started.md)
- [Architecture](docs/architecture.md)
- [Command Reference](docs/command-reference.md)
- [Storage Layout](docs/storage-layout.md)
- [Event Model](docs/event-model.md)
- [Changepacks](docs/changepack.md)
- [Verification](docs/verification.md)
- [Policy](docs/policy.md)
- [Services](docs/services.md)
- [Security](docs/security.md)
- [Release Compliance](docs/release-compliance.md)

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the development workflow and release expectations.

## License

Licensed under the terms in [LICENSE](LICENSE).
