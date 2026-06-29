# Draft Safety Guarantees

## What Draft guarantees

**Checkpoint before every commit.**
Draft creates a snapshot of your working tree before every `draft commit`. If something goes wrong, `draft undo` restores the files exactly as they were.

**Conflicts block commits.**
If Git reports unresolved merge conflicts, Draft refuses to commit. You must resolve them first.

**No auto-push.**
Draft never pushes to any remote. All operations are local.

**No auto-commit.**
Draft never commits without explicit user confirmation.

**.draft/ is excluded from Git.**
Draft writes `/.draft/` to `.git/info/exclude` on `draft start` so the storage directory is never accidentally committed.

**Atomic writes.**
All `.draft/` files are written atomically via a temp-file-then-rename pattern to avoid partial writes on crash.

---

## What Draft does not guarantee

**Draft is not a backup tool.**
Checkpoints only capture files that Git considers changed (tracked and modified, added, or deleted). Untracked files that have never been added to Git are not checkpointed.

**Draft does not prevent bad commits.**
Draft surfaces risk and verification evidence, but it is your responsibility to review and act on them. A high-risk change can still be committed if you confirm.

**Draft does not verify correctness.**
`draft verify` runs the command you give it and records the exit code. It does not interpret test output or guarantee that your code is correct.

**Draft does not protect against disk failure.**
If the disk fails during a commit, recovery depends on standard filesystem recovery. Draft's atomic writes reduce but do not eliminate the risk of corruption.

**Hunk-level selection is not supported in v0.1.0.**
Draft operates at the file and change-group level. You cannot include part of a file and exclude the rest in this version.

---

## Limitations

- Requires Git to be installed and on `$PATH`.
- Only Git repositories are supported. No SVN, Mercurial, or plain directories.
- Detached HEAD state blocks `draft commit`.
- Binary files are checkpointed by content but shown as `[binary]` in diffs.
- Very large repositories (tens of thousands of files) may have slower `draft review` performance.

---

## Security

- Draft does not collect telemetry.
- Draft does not make network requests.
- Draft does not read or write `.git` internals beyond `info/exclude`.
- All data stays local inside `.draft/`.
