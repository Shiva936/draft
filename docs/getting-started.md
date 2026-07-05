# Getting Started

This guide walks through a complete Draft v0.3.2 workflow using only local files and the CLI.

## Create A Workspace

```bash
draft init
draft config set identity.username "Ada"
draft config set identity.email "ada@example.com"
```

`draft init` creates `.draft/`, writes default configuration, creates the event stream, prepares the object store, and builds the local index. Running it again fails safely so accidental reinitialization cannot rewrite workspace state.

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

## Create A ChangePack

```bash
draft create "parser cleanup"
draft list
draft pack
```

A ChangePack is Draft’s reviewable unit. It contains a patch reference, evidence references, task links, review decisions, approvals, risk results, verification results, save receipts, and provenance hashes.

## Verify And Review

```bash
draft verify -p <ChangePack-id-or-name>
draft risk -p <ChangePack-id-or-name>
draft review -p <ChangePack-id-or-name>
draft approve -p <ChangePack-id-or-name> --reason "verified locally"
```

Verification runs configured commands and stores stdout, stderr, exit code, and timing as evidence. Risk analysis records findings that policy can use. Approval is required before save when the default policy is active.

## Save

```bash
draft save -p <ChangePack-id-or-name>
draft receipt list
draft receipt show <receipt-id>
```

Save persists the approved ChangePack into `.draft/` and writes a receipt. If `hooks.save` is configured, Draft runs it only after approval and safety checks. If `.draft/` appears in the save candidate, Draft aborts, records a failed receipt, emits `save.completed` with failure status, and does not execute `hooks.save`.

## Roll Back

```bash
draft rollback <chk-id|pck-id|rcp-id>
```

Rollback infers the target type from the ID prefix. Rollback never restores `.draft/`.

## Inspect Events

```bash
draft event
draft event --raw
draft doctor
draft receipt verify --all
```

`draft event` is a readable timeline derived from the stored event stream. `draft event --raw` prints the underlying JSONL records for audit, debugging, replay, and tooling. `draft doctor` and `draft receipt verify --all` verify event, receipt, and transparency integrity.

## Optional Local Services

The CLI does not need a daemon. `draftd` exists for optional local live/background flows; it is not a hosted service and does not add remote synchronization.
