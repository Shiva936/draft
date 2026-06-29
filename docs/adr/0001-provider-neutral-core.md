# ADR 0001 — Provider-neutral core

**Status:** Accepted (v0.2.0)

## Context
Draft v0.1.0 called Git directly from `core/`, coupling all business logic to a single VCS. We want Draft to support multiple version-control systems.

## Decision
`core/` depends only on the provider-neutral traits and types in `core::vcs` (`VcsProvider`, `VcsRepository`, `ProviderDelta`, opaque `Provider*Id`s, ...). All provider-specific code lives under `providers/*`. Core never parses provider identifiers (e.g. Git SHAs) — they are opaque strings.

To avoid a `core → provider` dependency cycle, the provider **registry is assembled by clients** (CLI, `draftd`, tests) via the `draft-providers` aggregator crate. Core ships an empty `ProviderRegistry`.

## Consequences
- Core has no Git dependency; `grep` for `git` in `core/src` finds only the `"git"` provider-id string used during v0.1 migration.
- Adding a provider means adding a crate that implements the traits and registering it — no core changes.
