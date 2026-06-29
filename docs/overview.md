# Overview

Draft adds a **trustworthy review/verify/finalize layer** on top of whatever version-control system (or plain folder) you already use.

```
You / your agent edit files
        ↓
Draft groups changes, scores risk, runs verification, records review
        ↓
Finalization → the provider creates a native object (e.g. a Git commit)
        ↓
A durable receipt maps Draft changes → provider objects
```

## What Draft is
- A provider-neutral workspace before finalization.
- A Draft-owned, append-only operation log and durable receipts.
- A local-first tool: no network is required for core functionality, and nothing
  is uploaded by default.

## What Draft is not
- Not a Git replacement, not a hosted review service, not a VCS itself.

## Providers
- **Git** — complete reference provider.
- **Filesystem / jj / Mercurial / Pijul** — experimental; detection and capabilities are implemented, but most operations return clear, structured "unsupported" errors. Do not treat them as production-ready.
