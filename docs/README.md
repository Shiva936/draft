# Draft Documentation

Draft v0.3.0 is organized around local verified changepacks. The docs are written for users, agent authors, contributors, and maintainers of the open-source project.

## Start Here

- [Getting Started](getting-started.md) walks through a complete local workflow.
- [Command Reference](command-reference.md) documents CLI behavior.
- [Architecture](architecture.md) explains the crate and service boundaries.
- [Storage Layout](storage-layout.md) describes the `.draft/` store.
- [Security](security.md) documents the safety model and threat boundaries.
- [Release Compliance](release-compliance.md) tracks production-readiness against the v0.3.0 planning intent.

## Concept References

- [Changepacks](changepack.md)
- [Tasks And Runs](task-run-model.md)
- [Evidence](evidence.md)
- [Verification](verification.md)
- [Policy](policy.md)
- [Review Cockpit](tui.md)
- [Compare And Compose](compare-compose.md)
- [Receipts](receipts.md)
- [Rollback](rollback.md)
- [Events](event-model.md)
- [Services](services.md)
- [Configuration](config-rules.md)
- [Draft Ignore Rules](ignore-rules.md)
- [target.local](target-local.md)

## v0.3.0 Boundary

Draft v0.3.0 is local-first. It stores verified changepacks in `.draft/`, supports optional opaque `target.local` execution after approval, and reserves later extension points without implementing network or hosted-service behavior.

Draft does not read external tool metadata to decide what changed. The workspace scanner walks files directly and applies only Draft’s own rules plus the hard `.draft/` exclusion.
