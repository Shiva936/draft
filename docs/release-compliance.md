# v0.3.3 Release Compliance

This document is the public release checklist for Draft v0.3.3. It maps the
v0.3.3 release intent to implementation, tests, and documentation so maintainers
can decide whether a build is ready to publish. The working `plans/` directory
remains private repo-planning material; public traceability belongs in this
checklist, README, release notes, and docs.

## Current Verdict

The current tree is expected to satisfy the v0.3.3 local production-readiness
gates once the release commands below pass. Draft remains local-first and
daemonless by default. v0.3.3 finalizes the verified stable-base model —
stable base, `stable_head`, project-state verification, configurable save
finalization with disposal, close/gc maintenance, composition validation, the
`proto/` contract layer, and human-readable CLI output — on top of the v0.3.2
verified changepack system.

## Requirement Matrix

| Area | Status | Evidence |
| --- | --- | --- |
| Stable base and `stable_head` | Implemented | `draft init` creates the initial stable base and `stable_head` with an `InitialStableBaseCreated` receipt; `stable_head` metadata is hash-verified on every read and by `draft gc`. |
| Project-state verification | Implemented | Before `stable_head` advances, save re-verifies the composed final state (workspace hash, `.draft/` exclusion, pack evidence, previous head integrity, trust-ledger verification) and records `ProjectStateVerified` or `ProjectStateVerificationFailed`; failure preserves the pack. |
| Save finalization and modes | Implemented | `[save].pack_disposal` selects `merge_and_dispose` (default) or `dispose_only`; invalid values fail clearly; the pipeline records `SaveStarted`, hook events, verification events, `StableHeadAdvanced`, `SaveFinalized`, and `PackDisposed`/`PackDisposalFailed`. |
| Pack disposal | Implemented | Disposal is the final successful save step; disposed packs are removed from both stores, dropped from the affected-path index, and are no longer rollback targets (rollback points to the receipt). |
| `draft close` / `draft gc` | Implemented | Close refuses unsafe pending state without `--force` and never deletes project files; gc prunes disposed/orphaned metadata, rebuilds the stable-graph and affected-path indexes, and validates `stable_head`. |
| Composition | Implemented | `draft pack compose` / `draft compose` classify independent/dependent/conflicting packs, order dependencies topologically, fail cycles, compute a deterministic `composition_hash`, and record `CompositionCreated`/`CompositionVerified`/`CompositionFailed`. |
| `proto/` contracts | Implemented | 16 specs, 8 JSON schemas, and 13 test vectors with schema-validated positive/negative payload fixtures; conformance-tested in core tests. |
| Deterministic verification keys | Implemented | `verification_key` composes workspace, config, toolchain, command, and environment hashes; persisted in `verify.json` and the verification cache manifest. |
| Human-readable output | Implemented | Default output is human-readable everywhere; JSON only via `--json`, or `--raw` on `draft event` (JSONL). |
| v0.3.2 migration | Implemented | Opening a v0.3.2 `.draft/` initializes `stable_head`, preserves pending packs, disposes nothing, and records `MigrationCompleted`; covered by a smoke test. |
| Global and project `.draft/` stores | Implemented | `draft init --global` provisions global identity and signing state; `draft init` provisions project state and v0.3.3 migration paths. |
| Hard `.draft/` exclusion | Implemented | Central path guards reject `.draft/` paths across pack, diff, import/export, save, rollback, scan, watcher, LSIF, hash, and hook candidate paths. |
| Signed receipts | Implemented | Trust-relevant actions create Ed25519 receipts; `draft receipt verify <rcp_id>` and `draft receipt verify --all` verify signatures and linkage. |
| Event and transparency integrity | Implemented | Events are final when appended with their linked receipt id; events and transparency entries are hash-chained; tampering is covered by unit, smoke, and security fixture tests. |
| Pack manifest and lockfile | Implemented | Canonical v0.3.3 manifests and lockfiles store intent, hashes, lifecycle state, verification, LSIF, and receipt references. |
| Import/export | Implemented | `.draftpack` (format 2) export embeds content-addressed objects; import supports quarantine, the full imported lifecycle (`imported_quarantined → import_verified → import_approved → import_saved`), local re-verification from embedded content, conflict-checked content application on save, name conflict handling, dry-run, deterministic archives, and fail-closed path/archive/receipt/object validation. |
| Verification, risk, LSIF, and selection | Implemented | `draft verify <pck_id> --explain/--full/--fuzz` records risk (with the ML-ready feature vector), symbol impact, test selection, fuzz selection, and command evidence; policy escalates full/fuzz verification for configured intents. |
| Policy enforcement | Implemented | Field-level policy resolution (project > global > safe default) governs the save gate (approval, critical-risk block, high-risk approval, workspace re-verification) and verification escalation; malformed policy files fail closed. |
| Save and rollback safety | Implemented | Save requires verification, approval, a canonical risk report with no unresolved critical risk, current evidence, and safe paths; rollback accepts `chk_`, `pck_`, and `rcp_` targets (legacy and canonical signed receipts) and supports dry-run. |
| Pack algebra | Implemented | `draft pack inspect`, `depends`, `conflicts`, and `compose` operate on canonical pack metadata and patch evidence. |
| AG-UI Review Cockpit | Implemented | `draft cockpit` serves a loopback-only browser cockpit with real pack/risk/diff/event/receipt APIs and CSRF-protected mutations. |
| Adapters | Implemented/experimental | MCP, ACP client, A2A, and AG-UI are implemented; `acp-comm` (Agent Communication Protocol) is marked experimental. Unsafe MCP operations are refused, and `draft doctor --global` lists every adapter's status. |
| Benchmarks and fuzzing | Implemented | Criterion benchmarks cover workspace hashing (incl. 10k files, cached), composition at 1k packs, conflict detection, gc cleanup, events, signing, risk, and LSIF; cargo-fuzz targets cover parsers and security boundaries. |
| CLI without daemon | Implemented | The CLI invokes core directly and does not require `draftd`. |
| Release workflow | Implemented | Tag-driven GitHub Actions release workflow validates versions, builds supported targets, packages `draft` and `draftd`, publishes versioned GitHub Release assets with GitHub artifact attestations, and marks the newest successful release as latest. |
| Installers | Implemented | `install.sh` and `install.ps1` fetch GitHub Releases from `Shiva936/draft`, verify `SHA256SUMS`, install into user-local directories by default, handle upgrades, and fail clearly on unsupported systems. |
| Public documentation | Implemented | README, docs, release notes, and this checklist document the current v0.3.3 behavior and rejected legacy surfaces. |

## Release Gates

A v0.3.3 release candidate must pass:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace --all-targets`
- `cargo test --workspace --doc`
- `cargo bench` for benchmark visibility before tagging
- `scripts/validate-version.sh`
- shell syntax validation for `install.sh`
- PowerShell parser validation for `install.ps1` when PowerShell is available
- a CLI contract audit confirming `draft event` supports only `--raw`,
  `--page`, and `--limit`, and rejects `draft log`, `draft event -p`, and
  `draft event -l`
- a docs audit confirming public docs do not advertise removed event flags or
  claim stubbed behavior as complete
- cross-platform smoke testing on Linux, macOS, Windows, and WSL, especially
  hidden `.draft/` behavior and key-file permissions

The release workflow must publish immutable versioned archives and `SHA256SUMS`
to GitHub Releases, plus GitHub artifact attestations for provenance.
Installers must use GitHub Releases, not expiring workflow artifacts, as their
source of truth.

## Maintainer Notes

Draft v0.3.3 must remain local-first and offline-first. No P0 feature may require
a hosted service, remote transparency service, marketplace, cloud sync, or
credential exchange. Hooks are opaque local commands captured as receipt
evidence; they are not parsed or modeled as native external-tool operations.
