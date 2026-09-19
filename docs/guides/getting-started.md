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
draft pack checkpoint "before parser cleanup"
```

A checkpoint stores a snapshot of the current workspace content. Draft uses snapshots to determine what changed later. The scanner walks the workspace directly and always excludes `.draft/`.

## Make ChangePacks

Edit files by hand, through scripts, or through an agent. Draft does not care how files changed. To inspect the current delta:

```bash
draft status
```

Status compares the current workspace to the latest snapshot and reports added, modified, deleted, renamed, type-changed, and permission-changed files.

## Create A ChangePack

```bash
draft pack new "parser cleanup" --scope src/parser.rs
draft pack list
draft pack revision seal <cpk-id>
```

A ChangePack is the unit of proposed work. Sealing observes the project's current state and records it as an immutable revision; sealing the same state twice is the same revision, so a re-run is not a second thing to review.

## Evidence, assessment, gate, decision

```bash
draft pack evidence run <rpk-id>
draft pack assess <rpk-id> --risk low --rationale "small, covered by tests"
draft pack gates evaluate <rpk-id>
draft pack decide <rpk-id> --approve
```

Each of those is a separate act, and the separations are the point. Evidence says what was observed. An assessment says what somebody judged it to mean. A gate says whether the required conditions hold. A Decision authorizes — and still does not accept anything.

Every one of them binds one exact revision and never carries to another.

## Promote

```bash
draft promote <cpk-id> <rpk-id>
draft baseline show
draft baseline receipts
```

Promotion is the only command that changes what this project accepts. It requires an approving Decision citing a satisfied Gate over the exact revision, and it refuses — rather than rebases — a promotion decided against a Baseline the project has since moved past. It issues one signed receipt and appends the events that say what it did.

## Recover

```bash
draft recover run <chk-id|cpk-id|evt-id>
```

Rollback infers the target type from the ID prefix. Rollback never restores `.draft/`.

## Inspect Events

```bash
draft activity list
draft activity list --raw
draft doctor
draft doctor receipts --all
```

`draft activity list` is a readable timeline derived from the Activity Ledger. `draft activity list --raw` prints each logical record as JSON for audit, debugging, replay, and tooling. `draft doctor` and `draft doctor receipts --all` verify the Activity chain, the receipts, and the transparency chain.

## Optional Local Services

The CLI does not need a daemon. `draftd` exists for optional local live/background flows; it is not a hosted service and does not add remote synchronization.

## FAQ

### Does Draft Replace Git?

No. Draft is a local review and safety layer. It does not replace Git, Jujutsu, CI, editors, agents, deployment systems, or code hosts.

### Does Draft Create Commits Or Pull Requests?

No. Draft has no native commit, push, pull request, merge request, publish, or hosted review command. A user-owned hook can run a local shell command, but Draft treats that as opaque hook execution.

### Where Does Draft Store Data?

Draft stores local metadata under `.draft/`. Treat it as sensitive because it can contain file content, command output, evidence, receipts, and event history.

### What Is The Difference Between `draft activity list` And `draft activity list --raw`?

`draft activity list` renders a readable timeline from the Activity Ledger. `--raw` prints each logical record as JSON. Draft stores only the framed records; the timeline is derived.

### Can I Use Draft Offline?

Yes. Core CLI flows are local and do not require a network service.

### Is The Daemon Required?

No. The CLI works without `draftd`. Service crates support optional local background and live flows.

### What Should I Do Before Risky Work?

Run `draft pack checkpoint "before work"` so you have a clear rollback target.
