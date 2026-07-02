# Checkpoints

A checkpoint records a baseline snapshot of workspace content.

## Create A Checkpoint

```bash
draft checkpoint "before agent run"
```

Draft returns a `chk_` ID and records a receipt and event. Checkpoints are used to compute later workspace deltas and can be rollback targets.

## Rollback

```bash
draft rollback chk_<id>
```

Rollback restores workspace files toward the checkpoint snapshot and protects `.draft/`.

## Guidance

Create checkpoints before risky edits, long agent sessions, large refactors, or local experiments. A checkpoint is a Draft snapshot, not a VCS commit.

