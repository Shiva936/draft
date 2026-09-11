# Release Compliance

This matrix maps the Draft release contract to implementation, tests, documentation, and delivery gates. It is the public evidence required for a release-readiness decision; private planning files are not release evidence.

## Current Verdict

The source candidate implements the frozen contract and has local evidence for strict schema handling, dependency security, Rust behavior, and Console reproducibility. Publication remains gated on the required GitHub Actions matrix, five-target dry run, installer smoke tests, and review of the final committed diff.

## Requirement Matrix

| Area | Status | Evidence |
| --- | --- | --- |
| Version and schema consistency | Implemented | Every workspace crate and displayed product declares. Draft-owned payloads independently require numeric `schema_version: 1`; architecture and contract scanners enforce stable unversioned names. |
| Task-centered orchestration | Implemented | Canonical v1 task definitions, deterministic templates, execution lifecycle, candidate profiles, evidence, decisions, waivers, ownership, inbox items, and status views are implemented in their owning core domains and exposed through the CLI. |
| Promotion | Implemented | The Baseline advances only through a journalled Promotion, on an approving Decision citing a satisfied Gate over the exact revision, with a preallocated receipt and preallocated Activity events. |
| Strict cutover | Implemented and tested | Missing or malformed schema markers fail as validation/corruption, other numeric versions fail as unsupported, and authoritative bytes are never converted, repaired, or rewritten. Derived indexes rebuild only after authoritative v1 input validates. |
| Protected data and redaction | Implemented | Central path guards keep `.draft/` and protected content out of candidates, exports, logs, Change-workspace writes, and execution output; durable payload redaction has focused tests. |
| Evidence and gate readiness | Implemented | Evidence freshness, deterministic risk, approval invalidation, expiring waivers, protected-path checks, ownership state, and actionable blockers feed shared readiness/status views. |
| Project registry and Doctor | Implemented | The user-scoped registry uses one canonical envelope. Doctor distinguishes unsupported schema, corruption, validation, and recovery diagnostics without changing authoritative inputs. |
| Draft Console | Implemented | `draftd` supplies authoritative revisioned models and session-bound actions. `console/application/` is the typed Rust client and `console/tui/` has no core/project-filesystem dependency. The loopback web gateway embeds unchanged `console/dist/` assets; CI rebuilds the assets and rejects a dirty diff. |
| Extension boundary | Implemented | The unversioned extension-package contract uses literal schema v1, content hashes, atomic local installation, explicit enabled state, and typed trust provenance; Draft does not execute extension entrypoints or ship protocol bridges. Extension semantics live in the portable format crate and Draft Core; acquisition, trust and installed state live in the extension service. |
| Capability authorization | Implemented | Source trust, installation and capability authorization are separate audited decisions. Grants bind to one artifact by id, source, publisher, version and content digest, so every update requires reauthorization; superseded grants are retained. Unauthorized commands are removed before Draft's domain logic sees them. |
| Domain neutrality | Implemented | Draft Core carries no language, ecosystem or toolchain knowledge. Symbol extraction, verification commands, toolchain probes, fuzz discovery and ecosystem risk rules are contributed data, checked by `scripts/check-extension-architecture.sh` and proven by `core/tests/no_extension_capabilities.rs`. |
| Extension repository independence | Implemented | `/extensions/` is a separate workspace depending only on the published format crate. `scripts/check-platform-without-extensions.sh` builds and tests Draft with it deleted; `extensions/scripts/check-standalone.sh` validates, packages, signs and verifies it in isolation. |
| Dependency security | Clean locally | Ratatui 0.30.2 uses patched `lru` 0.18.2, `paste` is absent, Crossterm is unified at 0.29, and `crossbeam-epoch` is patched to 0.9.20. `cargo audit --deny warnings` passes against the current RustSec database. |
| Rust CI | Configured | Formatting, version checks, installer syntax, strict Clippy, workspace tests, doctests, and release builds run on Ubuntu, macOS, and Windows. |
| Frontend CI | Configured | A Linux Console job runs `npm ci`, TypeScript checking, high-severity npm audit, production build, and a clean-diff check for the committed `dist/`; release artifact builds depend on it. |
| Release artifacts | Configured; execution pending | The release workflow builds Linux x64/ARM64, macOS x64/ARM64, and Windows x64 archives, then emits `SHA256SUMS` and GitHub build-provenance attestations. A v0.3.4 dry run must pass before tagging. |
| Activity Ledger | Implemented | One authoritative framed, hash-chained file (`events/events.log`); a closed v1 vocabulary where every event names an audit-fact owner and a journal mechanism; one converter and one appender, enforced by `scripts/check-activity-event-ownership.sh` and `scripts/check-core-architecture.sh`. |
| Receipts | Implemented | A v1 receipt attests a Promotion, a publication outcome, or a resolution of one — nothing else. Envelopes are stored create-once under their preallocated `rcp_` id, signed over a domain-separated canonical message covering the payload _and_ the signer binding, and verified at three levels reported separately, where what cannot be determined reads `unknown`. |
| Public documentation | Implemented | README, guides, reference (including [the Draft Change Graph](reference/dcg.md)), internals, protocol specs, changelog, release notes, roadmap, and this matrix describe the command and safety boundary. |

## Required Release Gates

The committed candidate must pass:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace --all-targets`
- `cargo test --workspace --doc`
- `cargo build --workspace --release`
- `cargo audit --deny warnings`
- `scripts/validate-version.sh`
- `scripts/check-core-architecture.sh`
- `scripts/check-console-architecture.sh`
- `scripts/check-extension-architecture.sh`
- `scripts/check-sdk-contract-packages.sh`
- `scripts/check-portable-contract-closure.sh`
- `scripts/check-lock-discipline.sh`
- `scripts/check-lock-order.sh`
- `scripts/check-activity-event-ownership.sh`
- `scripts/check-draft-contract-names.sh`
- `scripts/check-contract-registry.sh`
- `scripts/check-docs-consistency.sh`
- `scripts/check-generated-console-contracts.sh`
- `scripts/check-platform-without-extensions.sh`
- `scripts/check-retired-architecture.sh`
- `extensions/scripts/check-standalone.sh`
- `bash -n scripts/validate-version.sh scripts/package-release.sh install.sh`
- `sh -n install.sh`
- the PowerShell parser check for `install.ps1` on Windows
- from `console/web/`: `npm ci`, `npm run typecheck`, `npm test`, `npm audit --audit-level=high`, `npm run build`, `npm run test:e2e`, then `git diff --exit-code -- console/dist`

The pull request matrix must pass on Ubuntu, macOS, and Windows. The tag must not be created until a dry-run release packages all five supported targets successfully.

## Publication Verification

After merging the reviewed candidate, create `v0.x.x` and verify the GitHub Release contains exactly the five platform archives plus `SHA256SUMS`, with provenance attestations for the payload. Smoke-test `install.sh` and `install.ps1`, then confirm `draft --version`, `draftd --version`, initialization, Console startup, and one evidence/decide/promote workflow from installed binaries.

The current approximately 884 KB embedded Console bundle is accepted for v0.3.4 because it is local-only. Bundle-size optimization remains post-release work.
