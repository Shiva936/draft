# Git provider

The complete reference provider (`providers/git`). All Git execution flows through a single wrapper (`command.rs`) using **structured argument arrays** (no shell strings), capturing stdout/stderr and returning structured errors.

## Mapping
- `ProviderRevisionId` ← Git commit SHA (opaque to core)
- `ProviderStatus` ← `git status --porcelain=v1`
- `ProviderDelta` ← `git diff --binary` (added/modified/deleted/renamed/copied/ binary/untracked/conflicted); large text diffs are summarized
- finalization → a real `git commit`; the new SHA is returned as a `ProviderObjectRef` of kind `commit`

## Checkpoints
Non-destructive snapshots via `git stash create` (builds a commit object without touching the working tree or stash stack). Restore applies the snapshot and may require a clean-ish tree, otherwise it returns a safety error.

## Undo
`undo_provider_action` undoes the most recent finalization only if it is still `HEAD`, using `git reset --soft HEAD^` (content preserved, unstaged). Older commits are not rewritten; use `git revert` manually.

## `.draft/` exclusion
On init the provider adds `.draft/` to `.git/info/exclude` (never the project `.gitignore`), so Draft metadata is not finalized into history.
