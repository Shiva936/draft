# ADR 0003: Draft Event Log

## Status

Accepted for v0.3.0.

## Context

Draft needs an audit trail for local workspace actions without depending on any external history system. The log must be simple to inspect, append-only in normal operation, and tamper-evident.

## Decision

Draft stores events as JSON Lines under `.draft/events/`. Each event includes a previous event hash and its own hash. Appends are serialized with a local writer lock and synced after write.

## Consequences

- Events can be streamed by services and inspected by users.
- `draft events --verify-chain` can detect edits and broken links.
- The event log is not a backup system and does not provide signing in v0.3.0.
