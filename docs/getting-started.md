# Getting Started

This guide walks through a complete Draft v0.3.1 workflow using only local files and the CLI.

## Create A Workspace

```bash
draft init
draft config set identity.username "Ada"
draft config set identity.email "ada@example.com"
```

`draft init` creates `.draft/`, writes default configuration, creates the event log, prepares the object store, and builds the local index. Running it again is safe; Draft reports the existing workspace.

## Capture A Baseline

```bash
draft checkpoint "before parser cleanup"
```

A checkpoint stores a snapshot of the current workspace content. Draft uses snapshots to determine what changed later. The scanner walks the workspace directly and always excludes `.draft/`.

## Make Changes

Edit files by hand, through scripts, or through an agent. Draft does not care how files changed. To inspect the current delta:

```bash
draft status
```

Status compares the current workspace to the latest snapshot and reports added, modified, deleted, renamed, type-changed, and permission-changed files.

## Create A Changepack

```bash
draft create "parser cleanup"
draft list
draft pack
```

A changepack is Draft’s reviewable unit. It contains a patch reference, evidence references, task links, review decisions, approvals, risk results, verification results, save receipts, and provenance hashes.

## Verify And Review

```bash
draft verify <pack-id>
draft risk <pack-id>
draft review <pack-id>
draft approve <pack-id> --reason "verified locally"
```

Verification runs configured commands and stores stdout, stderr, exit code, and timing as evidence. Risk analysis records findings that policy can use. Approval is required before save when the default policy is active.

## Save

```bash
draft save <pack-id>
draft receipt list
draft receipt show <receipt-id>
```

Save persists the approved changepack into `.draft/` and writes a receipt. If `hooks.save` is configured, Draft runs it only after approval and safety checks. If `.draft/` appears in the save candidate, Draft aborts, records a failed receipt, emits `save.completed` with failure status, and does not execute `hooks.save`.

## Roll Back

```bash
draft rollback <chk-id|pck-id|rcp-id>
```

Rollback infers the target type from the ID prefix. Rollback never restores `.draft/`.

## Use The Daemon Optionaly

```bash
draft service status
draft service start
draft service stop
```

The CLI does not need the daemon. `draftd` exists for local live/background flows such as long-running review cockpit sessions and background indexing.
