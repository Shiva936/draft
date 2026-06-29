# ADR 0003 — Draft-owned operation log

**Status:** Accepted (v0.2.0)

## Context
Provider-native histories (Git reflog, jj op log, Mercurial journal) differ and are not portable. Draft needs a uniform, provider-independent audit trail.

## Decision
`core::operations` is an append-only, integrity-checked log under `.draft/operations/` (`NNNNNNNNNNNNNNNN.operation.json` + a rebuildable `index.json`). Each record carries actor, provider id, kind, refs, and a sha256 integrity hash (structured for future signing, unsigned in v0.2.0). Provider-native logs may be *referenced* in receipts but are never Draft's
source of truth.

## Consequences
- Major actions append operations; the orchestration layer (`core::app`) owns the records the engines don't self-append (see `core::app` docs).
- Indexes are rebuildable from the source records.
