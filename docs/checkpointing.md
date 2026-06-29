# Checkpointing

A checkpoint is a safety snapshot. `core::checkpoint` owns the metadata and delegates the snapshot mechanics to the provider.

- `draft checkpoint [-m MSG]` creates one; metadata persists under `.draft/checkpoints/` and a `CheckpointCreated` operation is appended.
- A pre-finalization checkpoint is created automatically when the provider supports local checkpoints, and is referenced by the finalization receipt for undo.
- **Git**: non-destructive `git stash create` snapshot; restore applies it and may require a clean-ish tree, else returns a safety error.
- Restore is supported only where safe; otherwise a structured error is returned.
