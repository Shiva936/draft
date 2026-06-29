# Architecture

```
CLI / TUI
   ↓ (IPC when draftd is running; embedded otherwise)
services/draftd  ──→  core::App  ──→  core engines
                                   ↓
                          core::vcs registry
                                   ↓
        providers/{git, fs, jj, mercurial, pijul}
```

## Crates
- `core` (`draft-core`) — provider-neutral model + engines + `core::App` orchestration. No provider code.
- `providers/*` — concrete providers implementing `VcsProvider`/`VcsRepository`.
- `providers/registry` (`draft-providers`) — assembles the default registry so `core` never depends on providers (no cycle).
- `services/*` — `ipc`, `store`, `locks`, `watcher`, `sessions`, `sync`, and the `draftd` binary.
- `cli` (`draft`), `tui` — clients of the same `core::App` API.

## Key boundary rules
- No Git (or any provider) code in `core`, `cli`, `tui`, or `services`.
- `draftd` coordinates; it owns no product logic.
- `core::App` is the single orchestration entry point and the only place that writes the operation records the engines don't self-append.
