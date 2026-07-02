# Draft Documentation

Draft v0.3.1 is organized around local verified ChangePacks. The docs are written for users, agent actors, contributors, and maintainers of the open-source project.

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
- [Release Compliance](release-compliance.md) tracks production-readiness against the v0.3.1 planning intent.

## Concept References

- [ChangePacks](changepack.md)
- [Checkpoints](checkpoints.md)
- [Candidates And Tasks](candidates-and-tasks.md)
- [Tasks And Runs](task-run-model.md)
- [Evidence](evidence.md)
- [Verification](verification.md)
- [Review And Approval](review-and-approval.md)
- [Policy](policy.md)
- [Review Cockpit](tui.md)
- [Compare And Compose](compare-compose.md)
- [Receipts](receipts.md)
- [Rollback](rollback.md)
- [Events](event-model.md)
- [Services](services.md)
- [Configuration](configuration.md)
- [Configuration Rules](config-rules.md)
- [Draft Ignore Rules](ignore-rules.md)
- [Hooks](hooks.md)

## v0.3.1 Boundary

Draft v0.3.1 is local-first. It stores verified ChangePacks in `.draft/` and supports optional opaque `hooks.*` command execution without implementing network, hosted-service, or native external-action behavior.

Draft does not read external tool metadata to decide what changed. The workspace scanner walks files directly and applies only Draft’s own rules plus the hard `.draft/` exclusion.
