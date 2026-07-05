# v0.3.2 Release Compliance

This document is the public release checklist for Draft v0.3.2. It maps the
v0.3.2 release intent to implementation, tests, and documentation so maintainers
can decide whether a build is ready to publish. The working `plans/` directory
remains private repo-planning material; public traceability belongs in this
checklist, README, release notes, and docs.

## Current Verdict

The current tree is expected to satisfy the v0.3.2 local production-readiness
gates once the release commands below pass. Draft remains local-first and
daemonless by default, while adding signed changepacks, import/export safety,
receipt and transparency verification, AG-UI, adapters, LSIF evidence, fuzz
targets, and benchmarks.

## Requirement Matrix

| Area | Status | Evidence |
| --- | --- | --- |
<<<<<<< Updated upstream
| Native `.draft/` store | Implemented | Workspace metadata, config, object store, event stream, receipts, index, snapshots, tasks, runs, ChangePacks, evidence, and policies are stored below `.draft/`. |
| Hard `.draft/` exclusion | Implemented | Scanner, snapshots, save candidates, rollback plans, watcher paths, and hook execution guards exclude Draft metadata. |
| `.draft/.ignore` | Implemented | Draft has a dedicated ignore command and file. It is separate from any external tool configuration. |
| Native scanner | Implemented | Status and snapshots walk the workspace directly and include files that are not known to any external system. |
| Snapshots and checkpoints | Implemented | Checkpoint creates a snapshot plus receipt and event. |
| Tasks and runs | Implemented | Tasks persist under the store; spawn/run commands capture command evidence. |
| ChangePacks | Implemented | ChangePacks persist manifests, patch references, evidence references, decisions, receipts, and status. |
| Evidence | Implemented | Verification and spawn commands attach durable evidence objects and references. |
| Append-only events | Implemented with hardening | Events are hash-chained and appended under a local writer lock with durable sync. |
| Provenance hashes | Implemented | Snapshots, patches, evidence, receipts, and events include content hashes. |
| Verification | Implemented | Configured commands run locally with stdout, stderr, status, and receipts. |
| Risk and policy | Implemented | Risk findings and policy gates block unsafe save paths. |
| Review and approval | Implemented | Review, approve, and reject state transitions are persisted. |
| Compare and compose | Implemented | Text patches include stable hunk records; compare reports file and hunk overlaps; compose rejects incompatible changes and creates a new ChangePack from source patch data. |
| Save and receipts | Implemented | Draft saves approved ChangePacks into `.draft/` and records save receipts. |
| Rollback | Implemented | Rollback plans avoid Draft metadata; apply validates normalized paths and symlink parent escapes; regression tests cover unsafe restore paths. |
| Services control plane | Implemented | IPC dispatch covers v0.3.1 methods; durable job records support local scan, verify, risk, compose, save, rollback, and storage-maintenance flows. |
| CLI without daemon | Implemented | CLI invokes core directly and does not require `draftd`. |
| TUI cockpit | Implemented | TUI renders workspace, ChangePacks, files, blockers, receipt count, service mode, and action affordances through a testable terminal renderer. |
| Public documentation | Implemented | The docs cover user, operator, security, contributor, architecture, command, service, and release-compliance topics. |
| Command hooks | Implemented | `hooks.save` is opaque hook execution after save approval and safety checks; raw and rich hooks use `{{name}}` placeholders, `--var` dynamic variables, and receipt hook status fields. |
| Release workflow | Implemented | Tag-driven GitHub Actions release workflow validates versions, builds supported targets, packages `draft` and `draftd`, publishes versioned GitHub Release assets with GitHub artifact attestations, and marks the newest successful release as latest. |
| Installers | Implemented | `install.sh` and `install.ps1` fetch GitHub Releases from `Shiva936/draft`, verify `SHA256SUMS`, install into user-local directories by default, handle upgrades, and fail clearly on unsupported systems. |
=======
| Global and project `.draft/` stores | Implemented | `draft init --global` provisions global identity and signing state; `draft init` provisions project state and v0.3.2 migration paths. |
| Hard `.draft/` exclusion | Implemented | Central path guards reject `.draft/` paths across pack, diff, import/export, save, rollback, scan, watcher, LSIF, and hook candidate paths. |
| Signed receipts | Implemented | Trust-relevant actions create Ed25519 receipts; `draft receipt verify <rcp_id>` and `draft receipt verify --all` verify signatures and linkage. |
| Event and transparency integrity | Implemented | Events are final when appended with their linked receipt id; events and transparency entries are hash-chained; tampering is covered by unit, smoke, and security fixture tests. |
| Pack manifest and lockfile | Implemented | Canonical v0.3.2 manifests and lockfiles store intent, hashes, lifecycle state, verification, LSIF, and receipt references. |
| Import/export | Implemented | `.draftpack` (format 2) export embeds content-addressed objects; import supports quarantine, the full imported lifecycle (`imported_quarantined → import_verified → import_approved → import_saved`), local re-verification from embedded content, conflict-checked content application on save, name conflict handling, dry-run, deterministic archives, and fail-closed path/archive/receipt/object validation. |
| Verification, risk, LSIF, and selection | Implemented | `draft verify <pck_id> --explain/--full/--fuzz` records risk (with the ML-ready feature vector), symbol impact, test selection, fuzz selection, and command evidence; policy escalates full/fuzz verification for configured intents. |
| Policy enforcement | Implemented | Field-level policy resolution (project > global > safe default) governs the save gate (approval, critical-risk block, high-risk approval, workspace re-verification) and verification escalation; malformed policy files fail closed. |
| Save and rollback safety | Implemented | Save requires verification, approval, a canonical risk report with no unresolved critical risk, current evidence, and safe paths; rollback accepts `chk_`, `pck_`, and `rcp_` targets (legacy and canonical signed receipts) and supports dry-run. |
| Pack algebra | Implemented | `draft pack inspect`, `depends`, `conflicts`, and `compose` operate on canonical pack metadata and patch evidence. |
| AG-UI Review Cockpit | Implemented | `draft cockpit` serves a loopback-only browser cockpit with real pack/risk/diff/event/receipt APIs and CSRF-protected mutations. |
| Adapters | Implemented/experimental | MCP, ACP client, A2A, and AG-UI are implemented; `acp-comm` (Agent Communication Protocol) is marked experimental. Unsafe MCP operations are refused, and `draft doctor --global` lists every adapter's status. |
| Benchmarks and fuzzing | Implemented | Criterion benchmarks cover hot paths; cargo-fuzz targets cover parsers and security boundaries. |
| Public documentation | Implemented | README, docs, release notes, and this checklist document the current v0.3.2 behavior and rejected legacy surfaces. |
>>>>>>> Stashed changes

## Release Gates

A v0.3.2 release candidate must pass:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
<<<<<<< Updated upstream
- `cargo test --workspace`
- `cargo test --workspace --doc`
- `scripts/validate-version.sh`
- shell syntax validation for `install.sh`
- PowerShell parser validation for `install.ps1` when PowerShell is available
- a text audit for prohibited external-product UX in command help and public docs;
- a text audit confirming no tracked file documents repo ignore-list entries;
- cross-platform smoke testing on Linux, macOS, Windows, and WSL.

The release workflow must publish immutable versioned archives and `SHA256SUMS` to GitHub Releases, plus GitHub artifact attestations for provenance. Installers must use GitHub Releases, not expiring workflow artifacts, as their source of truth.

The local verification run for this tree passes formatting, linting, workspace tests, daemon IPC tests, TUI render tests, core production tests, and both text
audits.
=======
- `cargo test --workspace --all-targets`
- `cargo bench` for benchmark visibility before tagging
- a CLI contract audit confirming `draft event` supports only `--raw`,
  `--page`, and `--limit`, and rejects `draft log`, `draft event -p`, and
  `draft event -l`
- a docs audit confirming public docs do not advertise removed event flags or
  claim stubbed behavior as complete
- cross-platform smoke testing on Linux, macOS, Windows, and WSL, especially
  hidden `.draft/` behavior and key-file permissions
>>>>>>> Stashed changes

## Maintainer Notes

Draft v0.3.2 must remain local-first and offline-first. No P0 feature may require
a hosted service, remote transparency service, marketplace, cloud sync, or
credential exchange. Hooks are opaque local commands captured as receipt
evidence; they are not parsed or modeled as native external-tool operations.
