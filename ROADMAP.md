# Roadmap

Draft is pre-1.0. This roadmap describes current direction without promising delivery dates.

## Current Focus: v0.3.4 — Task-Centered Change Orchestration

- Deterministic task definitions, candidate executions, evidence freshness, durable decisions, expiring waivers, and an attention inbox.
- Submit-readiness views that combine risk, verification, approval, ownership, evidence, waiver, and protected-file state.
- `draft submit` as verified finalization with configurable disposal (`merge_and_dispose` / `dispose_only`) and compact receipt/event provenance.
- Browser and terminal review under Draft Console, with a committed reproducible frontend embedded in the local service.
- Transactional v0.3.x migration to submit-named configuration and pack state, with byte-preserving backups and rollback on failure.
- Local-first and daemon-optional operation; `.draft/` remains hard-excluded and no hosted feature is required.

## Next: v0.4.0 — DraftHub (deferred by design)

- Remote changepack hosting, review, CI runners, CD, deployment receipts, and environment heads (`staging_head`, `production_head`) building on v0.3.4's task, evidence, receipt, hash, and `stable_head` contracts.

## Near-Term Areas

- Better human-readable summaries for ChangePacks, receipts, and event timelines.
- More focused tests around rollback, receipts, hooks, and storage maintenance.
- Console usability improvements.
- Clearer diagnostics for policy blockers and failed verification.

## Explicit Non-Goals

- Replacing Git or other VCS tools.
- Hosted review, hosted collaboration, pull requests, or merge queues (until DraftHub).
- Native remote sync, push, publish, or deployment behavior in v0.3.x.
- Inferring external tool semantics from hook command strings.
