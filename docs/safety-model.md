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

Default policy requires verification and approval before save, blocks save on an unresolved critical risk (or a missing canonical risk report), and requires re-verification when the workspace changed since verification. Risk findings can require human approval. Policy resolves field by field: project `.draft/policy.toml` over the global default policy over the built-in safe default.

## Import Boundary

Imported `.draftpack` artifacts are untrusted. They enter quarantine with all origin trust marks stripped, must be locally re-verified from their embedded content-addressed objects and approved, and are applied to the workspace only when every touched file matches the change's recorded base version. A checkpoint is created before any import content is applied, so the apply is always rollback-safe. Rejecting an import is terminal.

## Event Integrity

Events are hash-chained and receipt-linked. Use:

```bash
draft doctor
draft receipt verify --all
```

This detects parse failures, edited records, broken hash links, bad receipt signatures, and transparency-chain tampering.

## Hooks

Hooks are local shell commands configured by the user. Draft captures results and receipts but does not sandbox commands or infer external system semantics.

## Non-Goals

Draft v0.3.3 does not provide hosted collaboration, pull requests, native merge behavior, remote sync, deployment, marketplace behavior, or credential exchange.
