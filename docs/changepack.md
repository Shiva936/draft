# Changepacks

A changepack is Draft’s reviewable unit of change. It is native to Draft and stored under `.draft/changepacks/`.

## Purpose

Changepacks let a user or agent collect a workspace delta, attach evidence, run verification, record review decisions, and save the result with a receipt.

## Lifecycle

1. Create a checkpoint.
2. Edit the workspace.
3. Create a changepack from the working tree.
4. Attach evidence through spawn or verification.
5. Run risk and policy checks.
6. Review, comment, approve, or reject.
7. Save after policy allows it.
8. Use receipts and rollback events for audit and recovery.

## Manifest Fields

A changepack manifest includes:

- schema version;
- changepack id;
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
- source pack ids for composed packs;
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

Composition creates a new changepack from compatible sources. Overlapping file changes are rejected to prevent silently merging conflicting work.
