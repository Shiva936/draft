# Roadmap

Draft is pre-1.0. This roadmap describes current direction without promising delivery dates.

## Current Focus: v0.3.4 — Frozen Canonical Contract

- Deterministic task definitions, candidate executions, evidence freshness, durable decisions, expiring waivers, and an attention inbox.
- Submit-readiness views that combine risk, verification, approval, ownership, evidence, waiver, and protected-file state.
- `draft submit` as verified finalization with configurable disposal (`merge_and_dispose` / `dispose_only`) and compact receipt/event provenance.
- Browser and terminal review under Draft Console, with a committed reproducible frontend embedded in the local service.
- Compile-time-closed, independently evolvable contracts that all support only `schema_version: 1` in v0.3.4, with no alternate readers, converters, repair-on-open behavior, runtime registration, or compatibility façades beyond the intentional `/api/v1/...` Console HTTP boundary.
- Immutable pack revisions, revision-bound evidence, ledger-scoped signed events, deterministic source views, and distinct extension trust provenance.
- Local-first and daemon-optional operation; `.draft/` remains hard-excluded and no hosted feature is required.

## Future : v0.4.0 — Remaining Work (deferred by design)

- Remote pack hosting, review, CI runners, CD, deployment receipts, and environment heads (`staging_head`, `production_head`) building on  task, evidence, receipt, hash, and `stable_head` contracts.

## Near-Term Areas

- Better human-readable summaries for packs, receipts, and event timelines.
- More focused tests around rollback, receipts, hooks, and storage maintenance.
- Console usability improvements.
- Clearer diagnostics for policy blockers and failed verification.

## Explicit Non-Goals

- Replacing Git or other VCS tools.
- Hosted review, hosted collaboration, pull requests, or merge queues.
- Native remote sync, push, publish, or deployment behavior in v0.3.x.
- Inferring external tool semantics from hook command strings.
