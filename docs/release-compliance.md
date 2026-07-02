# v0.3.1 Release Compliance

This document is the public release checklist for Draft v0.3.1. It maps the planning intent to implementation, tests, and documentation so maintainers can
decide whether a build is ready to publish.

## Current Verdict

The current tree satisfies the v0.3.1 local production-readiness gates tracked by this document. The core flows are Draft-native, daemon IPC coverage includes
the control plane and durable service jobs, the TUI exposes review cockpit sections, and the release test/audit commands pass locally.

## Requirement Matrix

| Area | Status | Evidence |
| --- | --- | --- |
| Native `.draft/` store | Implemented | Workspace metadata, config, object store, event log, receipts, index, snapshots, tasks, runs, changepacks, evidence, and policies are stored below `.draft/`. |
| Hard `.draft/` exclusion | Implemented | Scanner, snapshots, save candidates, rollback plans, watcher paths, and hook execution guards exclude Draft metadata. |
| `.draft/.ignore` | Implemented | Draft has a dedicated ignore command and file. It is separate from any external tool configuration. |
| Native scanner | Implemented | Status and snapshots walk the workspace directly and include files that are not known to any external system. |
| Snapshots and checkpoints | Implemented | Checkpoint creates a snapshot plus receipt and event. |
| Tasks and runs | Implemented | Tasks persist under the store; spawn/run commands capture command evidence. |
| Changepacks | Implemented | Packs persist manifests, patch references, evidence references, decisions, receipts, and status. |
| Evidence | Implemented | Verification and spawn commands attach durable evidence objects and references. |
| Append-only events | Implemented with hardening | Events are hash-chained and appended under a local writer lock with durable sync. |
| Provenance hashes | Implemented | Snapshots, patches, evidence, receipts, and events include content hashes. |
| Verification | Implemented | Configured commands run locally with stdout, stderr, status, and receipts. |
| Risk and policy | Implemented | Risk findings and policy gates block unsafe save paths. |
| Review and approval | Implemented | Review, approve, and reject state transitions are persisted. |
| Compare and compose | Implemented | Text patches include stable hunk records; compare reports file and hunk overlaps; compose rejects incompatible changes and creates a new pack from source patch data. |
| Save and receipts | Implemented | Draft saves approved changepacks into `.draft/` and records save receipts. |
| Rollback | Implemented | Rollback plans avoid Draft metadata; apply validates normalized paths and symlink parent escapes; regression tests cover unsafe restore paths. |
| Services control plane | Implemented | IPC dispatch covers v0.3.1 methods; durable job records support scan, verify, risk, compose, save, rollback, and index rebuild. |
| CLI without daemon | Implemented | CLI invokes core directly and does not require `draftd`. |
| TUI cockpit | Implemented | TUI renders workspace, changepacks, files, blockers, receipt count, service mode, and action affordances through a testable terminal renderer. |
| Public documentation | Implemented | The docs cover user, operator, security, contributor, architecture, command, service, and release-compliance topics. |
| Command hooks | Implemented | `hooks.save` is opaque hook execution after save approval and safety checks; raw and rich hooks use `{{name}}` placeholders, `--var` dynamic variables, and receipt hook status fields. |

## Release Gates

A v0.3.1 release candidate must pass:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- a text audit for prohibited external-product UX in command help and public docs;
- a text audit confirming no tracked file documents repo ignore-list entries;
- cross-platform smoke testing on Linux, macOS, Windows, and WSL.

The local verification run for this tree passes formatting, linting, workspace tests, daemon IPC tests, TUI render tests, core production tests, and both text
audits.

## Maintainer Notes

Draft must remain local-only in v0.3.1. Hooks are opaque commands run only when the owning Draft command reaches the configured hook phase. They are captured as receipt evidence with `native_save_status`, `hook_status`, and `overall_status`; they are not parsed, detected, or modeled.
