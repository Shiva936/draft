# Conflict handling

`core::conflict` combines provider conflicts with Draft metadata conflicts into a single report used by status and the finalization gate.

- **Provider conflicts** — e.g. Git unmerged paths and conflict markers, surfaced via `VcsRepository::conflicts`.
- **Draft metadata conflicts** — e.g. a fresh operation-log append lock implies another Draft operation is in progress.

Finalization blocks on any unresolved conflict with a structured `ConflictDetected` error. Status shows the conflict count.
