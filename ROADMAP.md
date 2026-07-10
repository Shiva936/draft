# Roadmap

Draft is pre-1.0. This roadmap describes current direction without promising delivery dates.

## Current Focus: v0.3.3 — Stable Base Finalization

- Branchless stability: verified stable base states and `stable_head`, advanced
  only after project-state verification.
- `draft save` as verified finalization with configurable disposal
  (`merge_and_dispose` / `dispose_only`) and phased before/after hooks.
- Changepack disposal with compact provenance (receipts, events, indexes) so
  `.draft/` never becomes a duplicate history store.
- `draft close` and `draft gc` for safe metadata removal and maintenance.
- Composition validation: independent/dependent/conflicting packs, dependency
  ordering, deterministic composition hashes.
- Top-level `proto/` contract layer (specs, schemas, test vectors) and
  deterministic verification cache keys.
- Human-readable CLI output by default; JSON only via `--json`/`--raw`.
- Keep local-first and daemonless; `.draft/` always hard-excluded; no `draft log`.

## Next: v0.4.0 — DraftHub (deferred by design)

- Remote changepack hosting, review, CI runners, CD, deployment receipts, and
  environment heads (`staging_head`, `production_head`) building on v0.3.3's
  deterministic receipts, hashes, and `stable_head` metadata.

## Near-Term Areas

- Better human-readable summaries for ChangePacks, receipts, and event timelines.
- More focused tests around rollback, receipts, hooks, and storage maintenance.
- Review Cockpit usability improvements.
- Clearer diagnostics for policy blockers and failed verification.

## Explicit Non-Goals

- Replacing Git or other VCS tools.
- Hosted review, hosted collaboration, pull requests, or merge queues (until DraftHub).
- Native remote sync, push, publish, or deployment behavior in v0.3.x.
- Inferring external tool semantics from hook command strings.
