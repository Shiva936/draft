# Services

Draft services are optional local helpers. The CLI remains fully usable without them.

## `draftd`

`draftd` is the local control-plane process. It accepts IPC requests, dispatches to `draft-core`, and returns structured responses. It powers live/background flows for clients that need them.

Service responsibilities:

- receive local requests;
- validate request shape;
- dispatch core operations;
- stream or poll state for live clients;
- coordinate local sessions;
- expose service status.

The daemon does not change Draft’s authority model. Durable state still lives in `.draft/`.

## Durable Jobs

The service store records durable local jobs for scan, verify, risk, compose,
save, rollback, and index-rebuild requests. Jobs have queued, running,
completed, failed, and cancelled states. Job execution delegates to
`draft-core`, records the result or error, and keeps the CLI independent from
the daemon.

## IPC

The IPC crate provides local request/response transport. Unix platforms use a local socket path. Non-Unix platforms use a loopback fallback. Requests are JSON encoded and versioned by method names.

Daemon IPC tests cover:

- status dispatch;
- malformed requests;
- unknown methods;
- workspace operation errors;
- service lifecycle helpers;
- durable job submit, list, and status;
- fallback transport behavior.

## Watcher

The watcher debounces filesystem events and filters Draft write-back paths. It exists to trigger refreshes and background work, not to decide the authoritative workspace state. Core scanning remains authoritative.

## Locks

The locks service provides cross-platform local file guards. Draft uses locks for operations that must serialize local writers, including event append.

## Sessions

Sessions are lightweight in-memory records for connected clients. They are useful for accounting and ownership of local service requests.

## Store Registry

The service store records local daemon metadata so clients can discover service status.

## Protocol Adapters

Adapter status is explicit and honest — every adapter is implemented or marked
experimental, never a silent stub. `draft doctor --global` lists these
statuses. Adapter configuration lives under `~/.draft/adapters/<name>/`; no
adapter writes raw `.draft/` state or exposes signing keys.

| Adapter | Surface | Status |
| --- | --- | --- |
| MCP (Model Context Protocol) | `draft mcp` | Implemented |
| ACP client (Agent Client Protocol) | `draft acp` | Implemented |
| `acp-comm` (Agent Communication Protocol) | `draft acp` | **Experimental** |
| A2A (Agent2Agent) | `draft a2a` | Implemented |
| AG-UI Review Cockpit | `draft cockpit` | Implemented |

## Reserved Boundary

The `sync` crate is intentionally no-network in v0.3.3. It exists as a named boundary for later design work without changing current local-first behavior.
