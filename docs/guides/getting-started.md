# Getting Started

This guide walks through a complete Draft workflow using only local files and the CLI.

## Create A Workspace

```bash
draft init
draft config set user.name "Ada"
draft config set user.email "ada@example.com"
```

`draft init` creates `.draft/`, writes default configuration, creates the event stream, prepares the object store, and builds the local index. Running it again fails safely so accidental reinitialization cannot rewrite workspace state.

`user.name` and `user.email` are optional display/contact metadata, not security identity. A project value overrides the global value. An absent name resolves to the non-persisted fallback `unknown`; an absent email stays absent.

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

## Create A Pack

```bash
draft create "parser cleanup"
draft list
draft pack
```

A Pack is Draft’s reviewable unit. It contains a patch reference, evidence references, task links, review decisions, approvals, risk results, verification results, submit receipts, and provenance hashes.

## Verify And Review

```bash
draft verify -p <Pack-id-or-name>
draft risk -p <Pack-id-or-name>
draft review -p <Pack-id-or-name>
draft approve -p <Pack-id-or-name> --reason "verified locally"
```

Verification runs configured commands and stores stdout, stderr, exit code, and timing as evidence. Risk analysis records findings that policy can use. Approval is required before submit when the default policy is active.

## Submit

```bash
draft submit -p <Pack-id-or-name>
draft receipt list
draft receipt show <receipt-id>
```

Submit finalizes the approved Pack and writes a durable receipt. With the default `merge_and_dispose` mode, Draft verifies the resulting project state, advances `stable_head`, runs configured after-submit hooks, and disposes only mutable Pack staging. Immutable revisions and trust history remain. `dispose_only` delegates permanence to configured hooks and does not advance `stable_head`. A required hook failure preserves staging. If `.draft/` appears in the submit candidate, Draft aborts before hooks run.

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
