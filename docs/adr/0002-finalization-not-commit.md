# ADR 0002 — Finalization, not commit

**Status:** Accepted (v0.2.0)

## Context
"Commit" is a Git concept. Other providers create snapshots, changes, or patches. Draft's internal model must not assume Git semantics.

## Decision
Internally, Draft uses **finalization** (`core::finalization`): convert reviewed Draft changes into a provider-native object. The CLI keeps `draft commit` as a compatibility command that routes to the finalization engine. `draft finalize`
may be added as an alias later.

## Consequences
- `core::commit_engine` (v0.1) is replaced by `core::finalization`.
- Output reads "Finalized N Draft change(s) into <kind> <id>".
- Policy gates (review/risk/verification/conflict) live in finalization, not in the provider.
