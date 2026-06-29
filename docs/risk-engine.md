# Risk engine

Provider-neutral risk scoring over the diff (`core::risk`). Findings carry a severity; the change's level is the max finding.

## Rules
- secret-like patterns (private keys, AWS keys, hardcoded password/secret/token assignments) — **the raw value is never stored**
- large diffs (configurable line threshold)
- deletion-heavy changes (configurable file threshold)
- binary changes
- dependency/config file changes (Cargo.toml, package.json, lockfiles, ...)
- security-sensitive paths (auth, payment, credentials, `.env`, keys, ...)

## Levels
`Low < Medium < High < Critical`. Finalization can block on High/Critical via `[finalization].block_on_high_risk` (overridable with explicit confirmation). Tune via `[risk]` in `.draft/config.toml`.
