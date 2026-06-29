# Draft Limitations

Known limitations of Draft v0.1.0.

## Scope

**No hunk-level selection.**
Draft selects files at the change-group level. You cannot include lines from one hunk and exclude another within the same file. This is planned for a future version.

**No background monitoring.**
Draft does not watch the filesystem. You must run `draft review` manually before committing.

**No remote operations.**
Draft never pushes, fetches, or syncs with any remote. All operations are local.

**No multi-repo support.**
Draft operates on one Git repository at a time.

## Git compatibility

**Requires Git on PATH.**
Draft shells out to the `git` binary. Git must be installed and accessible.

**Detached HEAD blocks commit.**
`draft commit` is blocked when the repository is in detached HEAD state. Checkout a branch first.

**Shallow clones may behave unexpectedly.**
Shallow clones (`git clone --depth`) can cause `git show` to fail for checkpoint restore on deleted files. Full clones are recommended.

**Submodules are not handled.**
Changes inside Git submodules appear as a single diff entry and are treated as a single file group.

## Checkpoints

**Untracked files may not be captured.**
Only files appearing in `git status` output are snapshotted. Files that have never been `git add`-ed are not guaranteed to be in a checkpoint.

**No automatic checkpoint pruning.**
Checkpoints and blobs accumulate in `.draft/objects/blobs/`. Delete `.draft/` and re-run `draft start` to reset.

**Checkpoint restore does not undo the Git commit.**
`draft undo` restores the working tree to the pre-commit file state. It does not run `git revert` or `git reset`. The Git commit remains in history. To undo the Git commit itself, use `git reset HEAD~1` after `draft undo`.

## Verification

**Draft does not interpret test output.**
Draft records exit code and raw output. It does not parse test results, coverage, or lint output.

**Verification is advisory.**
A failed verification does not block `draft commit` by default. Use `--no-verify` only when intentional.

## Storage

**`.draft/` is local only.**
Draft state is not synced between machines. Each clone has its own independent `.draft/`.

**Schema migrations are not yet implemented.**
If the `.draft/` format changes between Draft versions, manual cleanup may be required.

## Platform

**Windows support is untested in v0.1.0.**
Draft is developed and tested on Linux and macOS. WSL is expected to work. Native Windows (`cmd.exe`, PowerShell) is not yet validated.

**Terminal color support required for TUI.**
The TUI requires a terminal that supports ANSI escape codes. Basic terminals without color support will fall back to text mode.
