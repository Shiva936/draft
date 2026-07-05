# Release Notes

See [CHANGELOG.md](CHANGELOG.md) for the public changelog.

## v0.3.2

Draft v0.3.2 makes Draft a verified changepack system. Where traditional version
control isolates timelines, Draft treats each changepack as an independent,
composable, portable, signed, and locally verifiable unit of change. It introduces secure
pack import/export with quarantine and content-embedded, applyable `.draftpack`
artifacts (format 2), the full imported-pack lifecycle (quarantine → local
re-verification → approval → conflict-checked save), an enforced field-level
policy layer governing the save and verification gates, Ed25519-signed receipts,
a tamper-evident event and transparency history (with rollback by canonical
signed receipt), hidden global/project `.draft/` metadata stores,
an AG-UI Review Cockpit, LSIF-backed semantic impact, an explainable rule-first
risk model that persists its ML-ready feature vector, and evidence-based
test/fuzz selection.

The goal is not to become another Git alternative — it is to be the safest local
workflow layer for reviewing human and AI-generated changes before they become
permanent. No `draft log`; use `draft event` with `--page`/`--limit`. IDs remain
`chk_`/`pck_`/`rcp_`.

## v0.3.1

Draft v0.3.1 introduces the local Verified ChangePacks + Review Cockpit model.

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
