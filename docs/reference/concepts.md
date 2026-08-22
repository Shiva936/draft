# Concepts

Draft v0.3.4 is organized around local, reviewable, signed ChangePacks.

## Workspace

A Draft workspace is a project directory with a `.draft/` store. Draft metadata is private project state and is always excluded from user change candidates.

## Checkpoints

A checkpoint records a baseline snapshot of workspace content. Later ChangePacks compare workspace changes against a snapshot.

Create a checkpoint before risky edits, long agent sessions, large refactors, or local experiments:

```bash
draft checkpoint "before agent run"
```

Draft returns a `chk_` ID and records a receipt and event. A checkpoint is a Draft snapshot, not a VCS commit, and can be used as a rollback target:

```bash
draft rollback chk_<id>
```

Rollback restores workspace files toward the checkpoint snapshot and protects `.draft/`.

## ChangePacks

A ChangePack is Draft's reviewable unit of change. It is stored under `.draft/changepacks/` while active and contains patch references, evidence, verification and risk results, review decisions, approval state, signed receipts, and provenance.

### Purpose And Lifecycle

ChangePacks let a user or agent collect a workspace delta, attach evidence, run verification, record review decisions, and submit the result with a receipt.

1. Create a checkpoint.
2. Edit the workspace.
3. Create a ChangePack from the working tree.
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

ChangePack IDs use the `pck_` prefix. Most ChangePack-targeting commands accept either an ID or a unique name with `-p`.

The review flow is:

```bash
draft verify -p <ChangePack>
draft risk -p <ChangePack>
draft review -p <ChangePack>
draft approve -p <ChangePack> --reason "reviewed"
draft submit -p <ChangePack>
```

### Selection And Deletion

```bash
draft pack -s <ChangePack>
draft pack -d <ChangePack>
```

Deleting a ChangePack preserves the event stream and receipts. Draft removes the pack directory, removes task and execution records owned only by that pack, and garbage-collects unreachable objects.

### Manifest And Patch Data

A ChangePack manifest includes:

- schema version;
- ChangePack ID and name;
- status and task ID;
- base snapshot ID;
- patch, evidence, verification, risk, decision, and receipt references;
- source ChangePack IDs for composed ChangePacks;
- actor and timestamps.

The patch records file-level changes and content hashes. Draft always rejects `.draft/` paths in submit candidates. Common lifecycle statuses include draft, verified, approved, rejected, submitted, and failed; transitions are policy checked. Successful submission disposes the active pack after durable receipts and finalization records have been written.

### Review Guidance

Reviewers should inspect:

- files changed;
- binary or deletion risk;
- evidence and verification output;
- policy blockers;
- comments and prior decisions;
- the submit receipt after completion.

ChangePacks are local Draft records, not commits, pull requests, hosted reviews, or merge requests. Hooks may call external tools after submit, but Draft does not model those tools natively.

## Imported Packs

A `.draftpack` artifact carries a pack's manifest, patch, evidence, signed receipts, and the content-addressed objects referenced by its patch. Imports are untrusted: they enter `imports/quarantine/`, lose all origin trust marks, and follow this lifecycle:

```text
imported_quarantined -> import_verified -> import_approved -> import_submitted
                                           \-> import_rejected
```

Rejection is terminal. Submitting an approved import applies content only when every touched file matches the change's recorded base version.

## Candidates, Tasks, And Executions

Candidates and tasks connect user intent, local execution profiles, and ChangePack provenance. Candidates are named execution profiles; they do not represent people, permissions, or hosted roles.

### Candidates

```bash
draft candidate add cli-helper --kind command -- "cargo test"
draft candidate list
draft candidate show cli-helper
draft candidate packs -c cli-helper
```

### Tasks

A task is a local record with an ID, title, optional description, actor, risk profile, optional linked issue text, status, and creation time. Tasks can link instructions, candidate profiles, ChangePacks, and evidence.

```bash
draft task spawn "agent edit" -c cli-helper -- "update parser error handling"
draft task list
draft task
```

A stored task can be spawned without repeating its instruction. An inline task requires text after `--`. Tasks can be linked to ChangePacks with `-p <ChangePack>` or through later review context.

### Executions

An execution records an opaque candidate command with its shell, working directory, status, timestamps, stdout and stderr object references, exit code, and linked task or ChangePack. Command candidates run in an isolated workspace copy and can produce a ChangePack against a shared baseline. Manual candidates queue a record for human work instead.

```bash
draft task spawn "agent edit" -c <candidate-name> -- <instruction>
```

An execution may produce file changes, while the ChangePack captures the final delta. Keeping both lets reviewers see the result and the process that produced it. Acceptability is still decided through ChangePack review, verification, risk, policy, and approval.

## Evidence

Evidence is durable context attached to tasks, executions, verification, and ChangePacks.

### Sources And Captured Data

Draft records evidence from:

- task and candidate execution provenance;
- `draft verify` command executions and summaries;
- risk and policy outputs;
- submit receipts and hook results.

Command evidence captures the command string or hash, shell, working directory, start and end times, exit code, stdout and stderr object references, and the related ChangePack or execution ID. Large payloads are stored as objects and referenced by hash.

Evidence should answer what was run, where and when it ran, what it returned, and how it affected submit readiness. Reviewers should treat missing evidence as a policy concern; the default submit policy requires verification.

Command output may include secrets, paths, source snippets, or machine details. Treat `.draft/` as sensitive when evidence was captured from private workspaces.

## Compare And Compose

Compare and compose help reviewers reason about multiple ChangePacks.

### Compare

```bash
draft compare <left-ChangePack> <right-ChangePack>
```

Compare reports files changed in each pack, files changed by both, a compatibility summary, and overlap warnings. Text patches contain stable hunk records. Same-file and hunk overlaps are reported separately so non-overlapping edits to the same file can be composed.

### Compose

```bash
draft compose <left-ChangePack> <right-ChangePack> --output "combined change"
```

Compose creates a new ChangePack from compatible source patch data. Draft rejects overlapping changes instead of guessing how to merge them. Before composing, verify both sources, inspect risk findings and overlaps, ensure the combined change has a coherent purpose, and verify the composed pack.

Composition records a receipt and event. The new manifest stores its source ChangePack IDs for provenance.

## Events And Receipts

Draft stores raw, hash-chained event records as JSON Lines under `.draft/events/`. `draft event` renders a human-readable timeline and `draft event --raw` prints the underlying JSONL. See [Storage And Events](../internals/storage-and-events.md).

A receipt is a signed durable record of a trust-relevant operation such as a checkpoint, verification, approval, submit, import or export, rollback, storage maintenance, or hook execution.

## Hooks

A hook is a user-configured shell command. Draft records hook execution but does not treat its contents as native Git, hosting, deployment, or remote behavior. See [Configuration](configuration.md#hooks).
