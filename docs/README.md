# Draft Documentation

Draft v0.3.3 is organized around local verified changepacks. The docs are written for users, agent actors, contributors, and maintainers of the open-source project.

## Start Here

- [Getting Started](getting-started.md) walks through a complete local workflow.
- [Installation](installation.md) covers source builds and CLI startup.
- [Concepts](concepts.md) defines workspaces, checkpoints, ChangePacks, events, receipts, candidates, tasks, and hooks.
- [Command Reference](command-reference.md) documents CLI behavior.
- [Architecture](architecture.md) explains the crate and service boundaries.
- [Storage Layout](storage-layout.md) describes the `.draft/` store.
- [Safety Model](safety-model.md) explains Draft’s local safety boundaries.
- [Security](security.md) documents the safety model and threat boundaries.
- [FAQ](faq.md) answers common public-project questions.
- [Release Compliance](release-compliance.md) tracks production-readiness against the v0.3.3 planning intent.

## Concept References

- [Protocol Contracts](protocol.md)
- [ChangePack Internals](changepack.md)
- [Checkpoints](checkpoints.md)
- [Candidates And Tasks](candidates-and-tasks.md)
- [Tasks And Runs](task-run-model.md)
- [Evidence](evidence.md)
- [Verification](verification.md)
- [Review And Approval](review-and-approval.md)
- [Policy](policy.md)
- [Review Cockpit](tui.md)
- [Compare And Compose](compare-compose.md)
- [Event Model Internals](event-model.md)
- [Services](services.md)
- [Config](config.md), [Configuration](configuration.md), and [Configuration Rules](config-rules.md)
- [Draft Ignore Rules](ignore-rules.md)
- [Hooks](hooks.md)

## Workflows

- [Draft With Git](git-workflows.md)
- [Draft-Only Workflows](draft-only-workflows.md)

## v0.3.3 Boundary

Draft v0.3.3 is local-first. It stores verified, signed, portable changepacks in `.draft/`, supports optional opaque `hooks.*` command execution, and does not implement network, hosted-service, marketplace, cloud sync, or native external-action behavior.

Draft does not read external tool metadata to decide what changed. The workspace scanner walks files directly and applies only Draft’s own rules plus the hard `.draft/` exclusion.
