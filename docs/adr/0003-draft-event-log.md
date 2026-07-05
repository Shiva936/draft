# ADR 0003: Draft Event Log

## Status

Accepted for v0.3.1.

## Context

Draft needs an audit trail for local workspace actions without depending on any external history system. The log must be simple to inspect, append-only in normal operation, and tamper-evident.

## Decision

Draft stores events as JSON Lines under `.draft/events/`. Each event includes a previous event hash and its own hash. Appends are serialized with a local writer lock and synced after write.

## Consequences

- Events can be streamed by services and inspected by users.
- v0.3.1 exposed direct event-chain verification; v0.3.2 verifies event integrity through `draft doctor` and `draft receipt verify --all`.
- The event stream is not a backup system. v0.3.2 links trust-relevant events to signed receipts and the local transparency chain.
