# Using Draft With Git

Draft does not require Git, but it integrates cleanly with it through save
hooks. Git is never invoked implicitly — only through hooks you configure.

## Pattern: Draft Verifies, Git Records

Use `dispose_only` mode so Draft gates the change and Git owns permanence:

```toml
[save]
pack_disposal = "dispose_only"

[hooks.save]
after = [{ command = "git add -A && git commit -m \"{{message}}\"" }]
```

Flow:

```text
draft create -> draft verify -> draft review/approve -> draft save
  -> before hooks run
  -> project-state verification passes
  -> after hook commits to Git
  -> Draft disposes the changepack
```

If the Git hook fails (non-zero exit), the save fails and the changepack is
preserved — Draft never assumes external permanence without a successful hook.

## Pattern: Draft And Git Side By Side

Keep the default `merge_and_dispose` and add Git hooks anyway: Draft advances
its own `stable_head` after verification, and the hook mirrors the change into
Git history. Draft's receipts stay the verification record; Git stays the
collaboration surface.

## Notes

- `.draft/` must never be committed; add it to `.gitignore`. Draft itself
  hard-excludes `.draft/` from packs, hashes, and saves.
- Hook template variables (`{{message}}`, `{{title}}`, `{{changepack_id}}`,
  `{{receipt_id}}`, …) are documented in [Hooks](hooks.md).
- Rollback (`draft rollback rcp_<id>`) only touches workspace files — your Git
  history is unaffected.

See [Draft-Only Workflows](draft-only-workflows.md) for using Draft without
Git entirely.
