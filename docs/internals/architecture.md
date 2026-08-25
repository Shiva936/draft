# Architecture

Draft is structured as a local-first workspace built in Rust:

```text
cli / tui / web
    |
optional draftd service
    |
draft-core::App
    |
global + project .draft/ durable stores
```

## Crate Boundaries

`core/` owns the domain model and durable stores. It implements config, scanning, snapshots, tasks, executions, packs, import/export, evidence, verification, risk, policy, review, approval, compare, compose, submit, signed receipts, rollback, events, transparency, object storage, LSIF, and indexing.

`cli/` exposes the command-line interface. It invokes `draft-core` directly so the CLI stays usable without a daemon.

`tui/` renders terminal review workflows from core state.

`console/` owns the browser Console boundary. Its Rust crate provides loopback HTTP transport, browser/session security, daemon integration, and embedded assets. `console/web/` owns React presentation source and `console/dist/` is the committed reproducible build. Console does not own Draft domain behavior.

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
4. A Pack captures the delta against a snapshot.
5. Verification and risk attach evidence and policy inputs.
6. Review decisions approve or reject the Pack.
7. Submit verifies the final project state, writes durable receipts, advances `stable_head` when configured, and disposes mutable staging while retaining immutable pack history.
8. Optional phased `hooks.submit` execution is captured as receipt evidence.
9. Every important transition appends a hash-chained event; trust-relevant transitions create signed receipts linked through the transparency chain.

## Store Authority

JSON and JSONL records are the durable source of truth. SQLite indexes are rebuildable caches. Object files are content-addressed by hash.

## Safety Boundary

`.draft/` is private Draft metadata. It is not a workspace change candidate. Any implementation that introduces `.draft/` into status, snapshots, Packs, submit, rollback, or external command execution is a release blocker.

## Local Services

Draft services are optional local helpers. The CLI invokes `draft-core` directly and remains fully usable without them.

### `draftd`

`draftd` is the local control-plane process. It accepts IPC requests, validates their shape, dispatches core operations, coordinates local sessions, and returns structured responses. It powers live or background flows for clients that need them without changing Draft's authority model.

### Durable Jobs

The service store records durable local jobs for scan, verify, risk, compose, submit, rollback, and index-rebuild requests. Jobs can be queued, running, completed, failed, or cancelled. Execution delegates to `draft-core` and stores the result or error.

### IPC

The IPC crate provides versioned, JSON-encoded local request/response transport. Unix platforms use a local socket path; non-Unix platforms use a loopback fallback. Tests cover status dispatch, malformed requests, unknown methods, workspace errors, lifecycle helpers, durable job operations, and fallback transport behavior.

### Watcher

The watcher debounces filesystem events and filters Draft write-back paths. It can trigger refreshes and background work, but core scanning remains the authoritative view of the workspace.

### Locks, Sessions, And Registry

The locks service supplies cross-platform local file guards for operations such as event append. Sessions are lightweight in-memory records for connected clients and request ownership. The service store records daemon metadata so clients can discover local service status.

### Draft Console Service

`draft console` starts the authenticated Console gateway on loopback. It serves pack, risk, receipt, task, inbox, Doctor, event, import/export, settings, and editor workflows without exposing signing keys. Mutations use typed daemon operations and the same core policy paths as the CLI, and require a per-session CSRF token. HTTP/browser translation and presentation stay in `console/`; all domain validation, transitions, persistence, and canonical operations stay in `draftd`, services, and `draft-core`.

The existing `~/.draft/adapters/agui/` namespace is reserved for intentional external AG-UI adapter configuration. It is unrelated to the browser Console package and ownership boundary.

Extension packages can be installed, inspected, enabled, disabled, and uninstalled with `draft extension`. Their entrypoints are not executable through the Draft CLI.

### Reserved Boundary

The `sync` crate is intentionally no-network in v0.3.4. It reserves a named boundary for later design work without changing current local-first behavior.
