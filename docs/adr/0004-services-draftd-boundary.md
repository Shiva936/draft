# ADR 0004: draftd Service Boundary

## Status

Accepted for v0.3.0.

## Context

Draft needs live/background flows for review cockpit sessions, indexing, scanning, and long-running local clients. At the same time, the CLI must stay reliable without a daemon.

## Decision

`draftd` is an optional local control-plane service. It dispatches IPC requests to `draft-core` and provides service-backed live behavior. Core remains authoritative and the CLI can call core directly.

## Consequences

- Users can run Draft in simple shell scripts without starting a service.
- TUI and long-running tools can use `draftd` when available.
- Service failure should not corrupt `.draft/` or make the CLI unusable.
