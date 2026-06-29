# ADR 0004 — `services/draftd` is coordination only

**Status:** Accepted (v0.2.0)

## Context
A daemon helps with watching, locking, and caching, but must not become a second home for product logic.

## Decision
`services/draftd` is a thin shell that dispatches IPC requests to the same `core::App` the CLI uses. Business logic stays in `core`, providers stay in `providers/*`. The daemon is **optional**: every safe command has an embedded fallback (NFR-006). IPC is local-only (Unix socket; user-only permissions).

## Consequences
- `draftd` depends on core + providers + the small service crates (ipc, store, locks, watcher, sessions, sync).
- CLI prefers the daemon when running, else runs embedded.
