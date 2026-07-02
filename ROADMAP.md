# Roadmap

Draft is pre-1.0. This roadmap describes current direction without promising delivery dates.

## Current Focus: v0.3.1

- Keep CLI flows local-first and daemonless by default.
- Maintain the ChangePack lifecycle: checkpoint, create, verify, risk, review, approve or reject, save, receipt, rollback.
- Keep `.draft/` hard-excluded from user change candidates.
- Improve documentation, command clarity, event UX, and release readiness.
- Keep hooks opaque and user-owned.

## Near-Term Areas

- Better human-readable summaries for ChangePacks, receipts, and event timelines.
- More focused tests around rollback, receipts, hooks, and storage maintenance.
- Review Cockpit usability improvements.
- Clearer diagnostics for policy blockers and failed verification.

## Explicit Non-Goals For v0.3.1

- Replacing Git or other VCS tools.
- Hosted review, hosted collaboration, pull requests, or merge queues.
- Native remote sync, push, publish, or deployment behavior.
- Inferring external tool semantics from hook command strings.

