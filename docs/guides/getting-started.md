# Getting Started

This guide walks through a complete Draft v0.3.4 workflow using only local files and the CLI.

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

A ChangePack is Draft’s reviewable unit. It contains a patch reference, evidence references, task links, review decisions, approvals, risk results, verification results, submit receipts, and provenance hashes.

## Verify And Review

```bash
draft verify -p <ChangePack-id-or-name>
draft risk -p <ChangePack-id-or-name>
draft review -p <ChangePack-id-or-name>
draft approve -p <ChangePack-id-or-name> --reason "verified locally"
```

Verification runs configured commands and stores stdout, stderr, exit code, and timing as evidence. Risk analysis records findings that policy can use. Approval is required before submit when the default policy is active.

## Submit

```bash
draft submit -p <ChangePack-id-or-name>
draft receipt list
draft receipt show <receipt-id>
```

Submit finalizes the approved ChangePack and writes a durable receipt. With the default `merge_and_dispose` mode, Draft verifies the resulting project state, advances `stable_head`, runs configured after-submit hooks, and disposes the active ChangePack metadata. `dispose_only` delegates permanence to configured hooks and does not advance `stable_head`. A required hook failure preserves the pack. If `.draft/` appears in the submit candidate, Draft aborts, records a failed receipt, emits `submit.completed` with failure status, and does not run submit hooks.

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

## FAQ

### Does Draft Replace Git?

No. Draft is a local review and safety layer. It does not replace Git, Jujutsu, CI, editors, agents, deployment systems, or code hosts.

### Does Draft Create Commits Or Pull Requests?

No. Draft has no native commit, push, pull request, merge request, publish, or hosted review command. A user-owned hook can run a local shell command, but Draft treats that as opaque hook execution.

### Where Does Draft Store Data?

Draft stores local metadata under `.draft/`. Treat it as sensitive because it can contain file content, command output, evidence, receipts, and event history.

### What Is The Difference Between `draft event` And `draft event --raw`?

`draft event` renders a readable timeline from stored event records. `draft event --raw` prints the underlying JSONL event envelopes. Draft stores only the raw event stream.

### Can I Use Draft Offline?

Yes. Core CLI flows are local and do not require a network service.

### Is The Daemon Required?

No. The CLI works without `draftd`. Service crates support optional local background and live flows.

### What Should I Do Before Risky Work?

Run `draft checkpoint "before work"` so you have a clear rollback target.
