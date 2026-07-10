# Architecture

Draft v0.3.3 is structured as a local-first Rust workspace:

```text
cli / tui
    |
optional draftd service
    |
draft-core::App
    |
global + project .draft/ durable stores
```

## Crate Boundaries

`core/` owns the domain model and durable stores. It implements config, scanning, snapshots, tasks, runs, changepacks, import/export, evidence, verification, risk, policy, review, approval, compare, compose, save, signed receipts, rollback, events, transparency, object storage, LSIF, and indexing.

`cli/` exposes the command-line interface. It invokes `draft-core` directly so the CLI stays usable without a daemon.

`tui/` renders the Review Cockpit. The TUI should be able to operate from core state directly or through service-backed live updates.

`services/` contains optional local services:

- `draftd`: IPC dispatcher and control plane;
- `ipc`: local request/response transport;
- `watcher`: debounced workspace notifications with Draft write-back filtering;
- `store`: local service registry records;
- `locks`: cross-platform local locks;
- `sessions`: connected-client accounting;
- `sync`: reserved no-network boundary for later design work.

## Data Flow

1. A user or agent changes workspace files.
2. Draft scans the workspace directly, excluding `.draft/`.
3. A checkpoint records a baseline snapshot.
4. A ChangePack captures the delta against a snapshot.
5. Verification and risk attach evidence and policy inputs.
6. Review decisions approve or reject the ChangePack.
7. Save records the verified ChangePack and receipt in `.draft/`.
8. Optional `hooks.save` execution is captured as receipt evidence.
9. Every important transition appends a hash-chained event and trust-relevant transitions create signed receipts linked through the transparency chain.

## Store Actority

JSON and JSONL records are the durable source of truth. SQLite indexes are rebuildable caches. Object files are content-addressed by hash.

## Safety Boundary

`.draft/` is private Draft metadata. It is not a workspace change candidate. Any implementation that introduces `.draft/` into status, snapshots, ChangePacks, save, rollback, or external command execution is a release blocker.
