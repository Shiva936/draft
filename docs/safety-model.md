# Safety Model

Draft’s safety model is local and conservative.

## Hard `.draft/` Exclusion

`.draft/` is Draft private metadata. Draft excludes it from:

- status;
- snapshots;
- ChangePacks;
- save candidates;
- rollback plans;
- watcher paths;
- hook candidate checks.

If `.draft/` appears in a save candidate, Draft aborts the save, records a failed receipt, emits a failed `save.completed` event, and skips `hooks.save`.

## Review Boundary

Default policy requires verification and approval before save. Risk findings can require human approval.

## Event Integrity

Events are hash-chained. Use:

```bash
draft event --verify-chain
```

This detects parse failures, edited records, and broken hash links.

## Hooks

Hooks are local shell commands configured by the user. Draft captures results and receipts but does not sandbox commands or infer external system semantics.

## Non-Goals

Draft v0.3.1 does not provide hosted collaboration, pull requests, native merge behavior, remote sync, deployment, or credential exchange.

