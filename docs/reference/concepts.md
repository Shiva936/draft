# Concepts

Draft is organized around local, reviewable, signed Packs.

## Workspace

A Draft workspace is a project directory with a `.draft/` store. Draft metadata is private project state and is always excluded from user change candidates.

## Checkpoints

A checkpoint records a baseline snapshot of workspace content. Later Packs compare workspace changes against a snapshot.

Create a checkpoint before risky edits, long agent sessions, large refactors, or local experiments:

```bash
draft checkpoint "before agent run"
```

Draft returns a `chk_` ID and records a receipt and event. A checkpoint is a Draft snapshot, not a VCS commit, and can be used as a rollback target:

```bash
draft rollback chk_<id>
```

Rollback restores workspace files toward the checkpoint snapshot and protects `.draft/`.

## Packs

A Pack is Draft's reviewable unit of change. It is stored under `.draft/packs/` while active and contains patch references, evidence, verification and risk results, review decisions, approval state, signed receipts, and provenance.

### Purpose And Lifecycle

Packs let a user or agent collect a workspace delta, attach evidence, run verification, record review decisions, and submit the result with a receipt.

1. Create a checkpoint.
2. Edit the workspace.
3. Create a Pack from the working tree.
4. Attach evidence through task execution or verification.
5. Run risk and policy checks.
6. Review, comment, approve, or reject.
7. Submit after policy allows it.
8. Use receipts and rollback events for audit and recovery.

### Create And Inspect

```bash
draft checkpoint "before work"
# edit files
draft create "parser cleanup"
draft list
draft pack
```

Pack IDs use the `pck_` prefix. Most Pack-targeting commands accept either an ID or a unique name with `-p`.

The review flow is:

```bash
draft verify -p <Pack>
draft risk -p <Pack>
draft review -p <Pack>
draft approve -p <Pack> --reason "reviewed"
draft submit -p <Pack>
```

### Selection And Deletion

```bash
draft pack -s <Pack>
draft pack -d <Pack>
```

Deleting a Pack preserves the event stream and receipts. Draft removes the pack directory, removes task and execution records owned only by that pack, and garbage-collects unreachable objects.

### Manifest And Patch Data

A Pack manifest includes:

- schema version;
- Pack ID and name;
- status and task ID;
- base snapshot ID;
- patch, evidence, verification, risk, decision, and receipt references;
- source Pack IDs for composed Packs;
- actor and timestamps.

The patch records file-level changes and content hashes. Draft always rejects `.draft/` paths in submit candidates. The lifecycle is `draft → verified → reviewing → approved → submitted`, with `reviewing → rejected`; transitions are policy checked. Successful submission disposes mutable staging after durable finalization while retaining the immutable pack and its history.

### Review Guidance

Reviewers should inspect:

- files changed;
- binary or deletion risk;
- evidence and verification output;
- policy blockers;
- comments and prior decisions;
- the submit receipt after completion.

Packs are local Draft records, not commits, pull requests, hosted reviews, or merge requests. Hooks may call external tools after submit, but Draft does not model those tools natively.

## Imported Packs

A `.draftpack` artifact carries a pack's manifest, patch, evidence, signed receipts, and the content-addressed objects referenced by its patch. Imports are untrusted: they enter `imports/quarantine/`, lose all origin trust marks, and follow this lifecycle:

```text
imported_quarantined -> import_verified -> import_approved -> import_submitted -> import_rejected
```

Rejection is terminal. Submitting an approved import applies content only when every touched file matches the change's recorded base version.

## Candidates, Tasks, And Executions

Candidates and tasks connect user intent, local execution profiles, and Pack provenance. Candidates are named execution profiles; they do not represent people, permissions, or hosted roles.

### Candidates

```bash
draft candidate add cli-helper --kind command -- "cargo test"
draft candidate list
draft candidate show cli-helper
draft candidate packs -c cli-helper
```

### Tasks

A task is a local record with an ID, title, optional description, actor, risk profile, optional linked issue text, status, and creation time. Tasks can link instructions, candidate profiles, Packs, and evidence.

```bash
draft task spawn "agent edit" -c cli-helper -- "update parser error handling"
draft task list
draft task
```

A stored task can be spawned without repeating its instruction. An inline task requires text after `--`. Tasks can be linked to Packs with `-p <Pack>` or through later review context.

### Executions

An execution records an opaque candidate command with its shell, working directory, status, timestamps, stdout and stderr object references, exit code, and linked task or Pack. Command candidates run in an isolated workspace copy and can produce a Pack against a shared baseline. Manual candidates queue a record for human work instead.

```bash
draft task spawn "agent edit" -c <candidate-name> -- <instruction>
```

An execution may produce file changes, while the Pack captures the final delta. Keeping both lets reviewers see the result and the process that produced it. Acceptability is still decided through Pack review, verification, risk, policy, and approval.

## Evidence

Evidence is durable context attached to tasks, executions, verification, and Packs.

### Sources And Captured Data

Draft records evidence from:

- task and candidate execution provenance;
- `draft verify` command executions and summaries;
- risk and policy outputs;
- submit receipts and hook results.

Command evidence captures the command string or hash, shell, working directory, start and end times, exit code, stdout and stderr object references, and the related Pack or execution ID. Large payloads are stored as objects and referenced by hash.

Evidence should answer what was run, where and when it ran, what it returned, and how it affected submit readiness. Reviewers should treat missing evidence as a policy concern; the default submit policy requires verification.

Command output may include secrets, paths, source snippets, or machine details. Treat `.draft/` as sensitive when evidence was captured from private workspaces.

## Compare And Compose

Compare and compose help reviewers reason about multiple Packs.

### Compare

```bash
draft compare <left-Pack> <right-Pack>
```

Compare reports files changed in each pack, files changed by both, a compatibility summary, and overlap warnings. Text patches contain stable hunk records. Same-file and hunk overlaps are reported separately so non-overlapping edits to the same file can be composed.

### Compose

```bash
draft compose <left-Pack> <right-Pack> --output "combined change"
```

Compose creates a new Pack from compatible source patch data. Draft rejects overlapping changes instead of guessing how to merge them. Before composing, verify both sources, inspect risk findings and overlaps, ensure the combined change has a coherent purpose, and verify the composed pack.

Composition records a receipt and event. The new manifest stores its source Pack IDs for provenance.

## Events And Receipts

Draft stores raw, hash-chained event records as JSON Lines under `.draft/events/`. `draft event` renders a human-readable timeline and `draft event --raw` prints the underlying JSONL. See [Storage And Events](../internals/storage-and-events.md).

A receipt is a signed durable record of a trust-relevant operation such as a checkpoint, verification, approval, submit, import or export, rollback, storage maintenance, or hook execution.

## Hooks

A hook is a user-configured shell command. Draft records hook execution but does not treat its contents as native Git, hosting, deployment, or remote behavior. See [Configuration](configuration.md#hooks).
