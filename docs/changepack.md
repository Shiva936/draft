# ChangePacks

A ChangePack is Draft’s reviewable unit of change. It is native to Draft and stored under `.draft/changepacks/`.

## Purpose

ChangePacks let a user or agent collect a workspace delta, attach evidence, run verification, record review decisions, and save the result with a receipt.

## Lifecycle

1. Create a checkpoint.
2. Edit the workspace.
3. Create a ChangePack from the working tree.
4. Attach evidence through task spawn or verification.
5. Run risk and policy checks.
6. Review, comment, approve, or reject.
7. Save after policy allows it.
8. Use receipts and rollback events for audit and recovery.

## Create And Inspect

```bash
draft checkpoint "before work"
# edit files
draft create "parser cleanup"
draft list
draft pack
```

ChangePack IDs use the `pck_` prefix. Most ChangePack-targeting commands accept either a ChangePack ID or a unique ChangePack name with `-p`.

## Review Flow

```bash
draft verify -p <ChangePack>
draft risk -p <ChangePack>
draft review -p <ChangePack>
draft approve -p <ChangePack> --reason "reviewed"
draft save -p <ChangePack>
```

## Selection And Deletion

```bash
draft pack -s <ChangePack>
draft pack -d <ChangePack>
```

Deleting a ChangePack does not delete the event stream, receipts, or other unrelated provenance.

## Manifest Fields

A ChangePack manifest includes:

- schema version;
- ChangePack id;
- name;
- status;
- task id;
- base snapshot id;
- patch references;
- evidence references;
- verification references;
- risk references;
- decision references;
- receipt references;
- source ChangePack IDs for composed ChangePacks;
- actor and timestamps.

## Patch Data

The patch records file-level changes and content hashes. Draft always rejects `.draft/` paths in save candidates.

## Statuses

Common statuses include draft, verified, approved, rejected, saved, and failed. Status transitions are policy checked.

## Review Guidance

Reviewers should inspect:

- files changed;
- binary or deletion risk;
- evidence and verification output;
- policy blockers;
- comments and prior decisions;
- save receipt after completion.

## Composition

Composition creates a new ChangePack from compatible sources. Overlapping file changes are rejected to prevent silently merging conflicting work.

## Boundaries

ChangePacks are local Draft records, not commits, pull requests, hosted reviews, or merge requests. Hooks may call external tools after save, but Draft does not model those tools natively.
