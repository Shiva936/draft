# Draft Documentation

Draft gives people control over agent-scale changes. It is organized around the local, verifiable Draft Change Graph. These documents serve users, agent actors, contributors, and maintainers of the open-source project.

## Guides

- [Installation](guides/installation.md) covers release binaries, supported platforms, upgrades, and source builds.
- [Getting Started](guides/getting-started.md) walks through a complete local workflow and answers common questions.
- [Workflows](guides/workflows.md) covers agent, Git-integrated, and Draft-only usage.
- [Console](guides/console.md) covers console lifecycle.

## Reference

- [The Draft Change Graph](reference/dcg.md) is the canonical model: the exact-reference audited set, immutable-fact integrity, provenance versus route, Baseline identity, and the staged-versus-dispatched attempt distinction.
- [Command Reference](reference/commands.md) documents the CLI surface.
- [Concepts](reference/concepts.md) explains workspaces, checkpoints, ChangePacks, tasks, executions, evidence, comparison, and composition.
- [Configuration](reference/configuration.md) covers config files, hooks, precedence, and ignore rules.
- [Review, Verification, And Policy](reference/review-and-policy.md) covers evidence gates, risk, approval, policy, and Draft Console.

## Internals

- [Architecture](internals/architecture.md) explains crate, service, daemon, and authority boundaries.
- [Storage And Events](internals/storage-and-events.md) describes `.draft/` stores, objects, receipts, and the Activity Ledger.
- [Security](internals/security.md) documents local trust and safety boundaries.
- [Protocol Contracts](internals/protocol.md) indexes canonical specifications, schemas, and conformance fixtures.

## Release

- [Release Compliance](release-compliance.md) maps release requirements to implementation, tests, and publication gates.

## Draft Boundary

Draft is local-first. It stores verified, signed, portable ChangePacks in `.draft/`, supports optional opaque `hooks.*` command execution, and does not implement network, hosted-service, marketplace, cloud-sync, or native external-action behavior.

Draft does not read external tool metadata to decide what changed. The workspace scanner walks files directly and applies only Draft's own rules plus the hard `.draft/` exclusion.
