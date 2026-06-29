# Architecture

Draft v0.3.0 is structured as a local-first Rust workspace:

```text
cli / tui
    |
optional draftd service
    |
draft-core::App
    |
.draft/ durable store
```

## Crate Boundaries

`core/` owns the domain model and durable store. It implements config, scanning, snapshots, tasks, runs, changepacks, evidence, verification, risk, policy, review, approval, compare, compose, save, receipts, rollback, events, object storage, and indexing.

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
4. A changepack captures the delta against a snapshot.
5. Verification and risk attach evidence and policy inputs.
6. Review decisions approve or reject the changepack.
7. Save records the verified changepack and receipt in `.draft/`.
8. Optional `target.local` execution is captured as receipt evidence.
9. Every important transition appends a hash-chained event.

## Store Authority

JSON and JSONL records are the durable source of truth. SQLite indexes are rebuildable caches. Object files are content-addressed by hash.

## Safety Boundary

`.draft/` is private Draft metadata. It is not a workspace change candidate. Any implementation that introduces `.draft/` into status, snapshots, changepacks, save, rollback, or external command execution is a release blocker.
