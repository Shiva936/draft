# Services and `draftd`

`draftd` is an **optional** local daemon that coordinates long-running behavior. It owns no product logic — it dispatches to the same `core::App` the CLI uses.

## Responsibilities
Workspace registry, local IPC, file watcher (debounced), session manager, locks, service store, provider-detection cache.

## IPC
Local-only, newline-delimited JSON-RPC over a Unix domain socket (`$XDG_RUNTIME_DIR/draft/draftd.sock`, falling back to
`~/.local/state/draft/draftd.sock`), with user-only permissions. Path parameters are validated and traversal is rejected.

Request / response:
```json
{ "id": "1", "method": "workspace.status", "params": { "path": "/repo" } }
{ "id": "1", "ok": true, "result": { ... } }
{ "id": "1", "ok": false, "error": { "code": "WORKSPACE_NOT_FOUND", "message": "..." } }
```

Methods: `service.ping`, `service.status`, `service.shutdown`, `provider.list`, `workspace.detect`, `workspace.status`, `workspace.register`, `receipt.list`.

## Optionality
If `draftd` is not running, the CLI runs embedded. Start/stop with `draft service start` / `draft service stop`. When started from inside an initialized Draft workspace, the CLI registers that workspace with the daemon; starting the service outside a workspace remains non-fatal.
