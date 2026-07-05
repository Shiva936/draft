# Roadmap

Draft is pre-1.0. This roadmap describes current direction without promising delivery dates.

## Current Focus: v0.3.2

- Verified changepack system: signed receipts, hash-chained events, transparency.
- Two hidden `.draft/` stores (global + project) with config/policy precedence.
- Portable `.draftpack` import/export with quarantine and hardened validation.
- Explainable risk, basic LSIF impact, and evidence-based test/fuzz selection.
- Pack algebra (inspect/depends/conflicts/compose), AG-UI cockpit, MCP/ACP/A2A.
- Keep local-first and daemonless; `.draft/` always hard-excluded; no `draft log`.

## Previous Focus: v0.3.1

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

## Explicit Non-Goals

- Replacing Git or other VCS tools.
- Hosted review, hosted collaboration, pull requests, or merge queues.
- Native remote sync, push, publish, or deployment behavior.
- Inferring external tool semantics from hook command strings.

