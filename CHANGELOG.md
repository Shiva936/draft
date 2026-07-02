# Changelog

All notable public changes to Draft are tracked here.

## v0.3.1

Draft v0.3.1 focuses on local verified ChangePacks, review, approval, receipts, rollback, and public documentation readiness.

### Added

- Native `.draft/` workspace store for config, objects, snapshots, ChangePacks, events, receipts, evidence, tasks, runs, and rebuildable indexes.
- Local ChangePack flow: checkpoint, create, verify, risk, review, approve or reject, save, receipt inspection, and rollback.
- Append-only hash-chained event stream with `draft event` and `draft event --verify-chain`.
- `draft event --raw` for JSONL event records and a normal human-readable timeline derived from those records.
- Candidate and task commands for local execution profiles and task provenance.
- Optional opaque `hooks.save` execution after Draft approval, policy, and `.draft/` safety checks.
- Storage maintenance commands and local service crates for optional background flows.

### Safety

- `.draft/` is hard-excluded from status, snapshots, ChangePacks, save candidates, rollback plans, watcher paths, and hook candidate checks.
- Failed saves record receipts and events.
- Hooks are captured as command evidence but are not interpreted as native Git, host, deployment, or remote operations.

### Documentation

- Public README, user guides, command reference, safety model, release compliance, support, conduct, brand, and roadmap documentation.

