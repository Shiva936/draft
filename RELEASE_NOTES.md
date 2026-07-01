# Release Notes

## v0.3.0

Draft v0.3.0 introduces the local Verified Changepacks + Review Cockpit model.

### Added

- Native `.draft/` workspace store.
- Draft config, `.draft/.ignore`, verification, and policy files.
- Native workspace scanner with direct file walking.
- Snapshots, checkpoints, tasks, runs, changepacks, evidence, review decisions, approvals, receipts, and rollback records.
- Append-only hash-chained events with verification.
- Content-addressed object storage.
- Rebuildable SQLite index.
- Verification, risk, policy, review, compare, compose, save, and rollback commands.
- Optional `hooks.save` execution after Draft save approval and safety checks.
- Local daemon IPC dispatch for service-backed flows.
- Public documentation for users, operators, contributors, and maintainers.

### Safety

- `.draft/` is hard-excluded from status, snapshots, changepacks, save candidates, rollback plans, and external command candidate checks.
- If `.draft/` appears in a save candidate, Draft aborts save, emits `SaveFailed`, records a failed receipt, and skips `hooks.save`.
- Hooks are opaque. Draft captures stdout, stderr, exit code, command hash, and receipt linkage without interpreting the command.
