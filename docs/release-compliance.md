# v0.3.4 Release Compliance

This matrix maps the Draft v0.3.4 release contract to implementation, tests, documentation, and delivery gates. It is the public evidence required for a release-readiness decision; private planning files are not release evidence.

## Current Verdict

The source candidate implements the v0.3.4 contract and has local evidence for migration safety, dependency security, Rust behavior, and Console reproducibility. Publication remains gated on the required GitHub Actions matrix, five-target dry run, installer smoke tests, and review of the final committed diff.

## Requirement Matrix

| Area | Status | Evidence |
| --- | --- | --- |
| Version and schema consistency | Implemented | Every workspace crate, CLI version string, protocol schema, and frontend package declares v0.3.4; `scripts/validate-version.sh` enforces consistency. |
| Task-centered orchestration | Implemented | Versioned task definitions, deterministic templates, execution lifecycle, candidate profiles, evidence, decisions, waivers, ownership, inbox items, and status views are implemented in `core/` and exposed through the CLI. |
| Submit finalization | Implemented | New state uses submit-named config, hooks, manifests, events, receipts, schemas, and lifecycle labels; historical save-named fields remain read-compatible, while the retired `draft save` command is absent. |
| Migration transaction | Implemented and tested | `draft doctor migrate` discovers every migration target, writes byte-for-byte backups preserving `.draft/` relative paths, validates all transformed workspace/config/manifest data before source writes, uses atomic replacements, and restores originals on commit or completion-event failure. Focused tests cover success, idempotency, stored and quarantined pending-pack preservation, malformed-manifest no-write behavior, and original backup bytes. |
| Protected data and redaction | Implemented | Central path guards keep `.draft/` and protected content out of candidates, exports, logs, editor writes, and execution output; durable payload redaction has focused tests. |
| Evidence and submit readiness | Implemented | Evidence freshness, deterministic risk, approval invalidation, expiring waivers, protected-path checks, ownership state, and actionable blockers feed shared readiness/status views. |
| Project registry and Doctor | Implemented | User-scoped registry sync, project/global index status, migration, journal recovery reporting, and storage stats/gc/compact/prune are exposed and documented. |
| Draft Console | Implemented | The loopback-only service embeds committed `services/agui/dist/` assets at compile time; the Console source supports type-checking and deterministic production builds. CI rebuilds the assets and rejects a dirty diff. |
| Extension boundary | Implemented | Versioned manifests, content hashes, atomic local installation, enabled state, and uninstall are supported; Draft does not execute extension entrypoints or ship protocol bridges. |
| Dependency security | Clean locally | Ratatui 0.30.2 uses patched `lru` 0.18.2, `paste` is absent, Crossterm is unified at 0.29, and `crossbeam-epoch` is patched to 0.9.20. `cargo audit --deny warnings` passes against the current RustSec database. |
| Rust CI | Configured | Formatting, version checks, installer syntax, strict Clippy, workspace tests, doctests, and release builds run on Ubuntu, macOS, and Windows. |
| Frontend CI | Configured | A Linux Console job runs `npm ci`, TypeScript checking, high-severity npm audit, production build, and a clean-diff check for the committed `dist/`; release artifact builds depend on it. |
| Release artifacts | Configured; execution pending | The release workflow builds Linux x64/ARM64, macOS x64/ARM64, and Windows x64 archives, then emits `SHA256SUMS` and GitHub build-provenance attestations. A v0.3.4 dry run must pass before tagging. |
| Public documentation | Implemented | README, guides, reference, internals, changelog, release notes, roadmap, and this matrix describe the v0.3.4 command and safety boundary. |

## Required Release Gates

The committed candidate must pass:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace --all-targets`
- `cargo test --workspace --doc`
- `cargo build --workspace --release`
- `cargo audit --deny warnings`
- `scripts/validate-version.sh`
- `bash -n scripts/validate-version.sh scripts/package-release.sh install.sh`
- `sh -n install.sh`
- the PowerShell parser check for `install.ps1` on Windows
- from `services/agui/web/`: `npm ci`, `npm run typecheck`, `npm audit --audit-level=high`, `npm run build`, then `git diff --exit-code -- services/agui/dist`

The pull request matrix must pass on Ubuntu, macOS, and Windows. The tag must not be created until a dry-run release packages all five supported targets successfully.

## Publication Verification

After merging the reviewed candidate, create `v0.3.4` and verify the GitHub Release contains exactly the five platform archives plus `SHA256SUMS`, with provenance attestations for the payload. Smoke-test `install.sh` and `install.ps1`, then confirm `draft --version`, `draftd --version`, initialization, Console startup, and one verify/approve/submit workflow from installed binaries.

The current approximately 884 KB embedded Console bundle is accepted for v0.3.4 because it is local-only. Bundle-size optimization remains post-release work.
