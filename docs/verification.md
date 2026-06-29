# Verification

`draft verify` runs configured commands and persists results under `.draft/verification/` (plus logs/). Commands are **always shown** before/while they run — Draft never runs verification silently (NFR §8.3).

- Configure in `.draft/config.toml`: `[[verification.commands]] name = "test" command = "cargo" args = ["test"]`.
- If none are configured, Draft infers one from the project (Cargo/go/npm/ pytest/make). You can override with `draft verify "<command>"`.
- Structured execution (no shell), per-command timeout, captured (truncated) output, and statuses: passed / failed / skipped / timed-out / cancelled.
- The latest result feeds the finalization verification gate.
