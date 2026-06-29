# Draft v0.2.0 — Release Notes

Draft v0.2.0 is a major architectural release that turns Draft from Git-coupled pre-commit tooling into a **provider-neutral, local-first collaboration workspace before finalization**.

## Highlights
- **Provider-neutral architecture** — `core/` no longer depends on Git; it depends only on the `core::vcs` provider abstraction.
- **Git provider extraction** — all Git logic now lives in `providers/git`, the complete reference provider. Core/CLI/TUI/services never call Git directly.
- **Draft operation log** — append-only, integrity-checked history under `.draft/operations/`, independent of provider-native logs.
- **Finalization model** — internal "commit" is replaced by finalization; `draft commit` remains as a compatibility command.
- **Receipts** — durable records mapping Draft changes → provider objects.
- **`services/draftd`** — an optional local daemon (IPC, watcher, locks, sessions, store). Every safe command has an embedded fallback.
- **Experimental providers** — `fs`, `jj`, `mercurial`, `pijul` scaffolds with declared capabilities and structured unsupported-operation errors. Not production-ready.
- **Migration from v0.1.0** — existing Git workspaces migrate automatically.
- **Public docs** — a full `docs/` set plus ADRs and spec copies.

## Known limitations
- jj / Mercurial / Pijul providers are experimental (detection + capabilities).
- Filesystem provider is limited (status scan only; no finalization).
- No cloud sync, hosted review, or real-time/CRDT collaboration.
- No Draft-native VCS; no cryptographic operation signing by default; no
  enterprise policy server.

## Compatibility
- v0.1.0 Git workflows (`status`, `review`, `verify`, `commit`, `undo`) are preserved. Existing `.draft/` metadata is migrated on first use; old metadata is backed up under `.draft/backup/`.

## Artifacts
- `draft` CLI binary and `draftd` daemon binary.
- `docs/`, this file, and `examples/config.toml`.

Version `0.2.0` is reported by `draft --version`, `draftd --version`, and the workspace `config.toml`.
