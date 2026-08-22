# Workflows

Draft is designed to make AI-generated changes reviewable before submit.

## Recommended Flow

```bash
draft checkpoint "before agent run"
draft task spawn "agent edit" -- <agent command>
draft status
draft create "agent change"
draft verify -p <pck-id>
draft risk -p <pck-id>
draft review -p <pck-id>
draft approve -p <pck-id> --reason "human reviewed"
draft submit -p <pck-id>
```

## Why Spawn Through Draft

`draft task spawn` records task intent, candidate links, optional ChangePack links, and instruction text. Command candidates run in an isolated workspace copy and can produce reviewable ChangePacks; manual candidates remain queued for human edits. This gives reviewers context for what the agent was asked to do and evidence about its execution before changes are reviewed.

## Review Checklist For Agent Changes

- Inspect every changed file.
- Read captured stdout and stderr when available.
- Run verification locally.
- Check risk findings for broad, binary, deletion, or sensitive-path changes.
- Require human approval for high-risk changes.
- Submit only after policy is satisfied.

## Failed Runs

A failed agent command can still produce useful evidence. Keep the run record, inspect the workspace, and decide whether to create a ChangePack or roll back.

## Using Draft With Git

Draft does not require Git, but it integrates with it through submit hooks. Git is never invoked implicitly—only through hooks you configure.

### Pattern: Draft Verifies, Git Records

Use `dispose_only` mode so Draft gates the change and Git owns permanence:

```toml
[submit]
pack_disposal = "dispose_only"

[hooks.submit]
after = [{ command = "git add -A && git commit -m \"{{message}}\"" }]
```

Flow:

```text
draft create -> draft verify -> draft review/approve -> draft submit
  -> before hooks run
  -> project-state verification passes
  -> after hook commits to Git
  -> Draft disposes the changepack
```

If the Git hook fails with a non-zero exit, submit fails and the ChangePack is preserved. Draft never assumes external permanence without a successful hook.

### Pattern: Draft And Git Side By Side

Keep the default `merge_and_dispose` mode and add Git hooks: Draft advances its own `stable_head` after verification, while the hook mirrors the change into Git history. Draft receipts remain the verification record and Git remains the collaboration surface.

### Git Notes

- `.draft/` must never be committed; add it to `.gitignore`. Draft itself hard-excludes `.draft/` from packs, hashes, and submits.
- Hook template variables such as `{{message}}`, `{{title}}`, `{{changepack_id}}`, and `{{receipt_id}}` are documented in [Configuration](../reference/configuration.md#placeholders).
- `draft rollback rcp_<id>` touches workspace files only; Git history is unaffected.

## Draft-Only Workflows

Draft works without Git or another VCS. In a Draft-only repository, verified stable base states are the project's record of permanence.

### The Draft-Only Loop

```text
draft init                       # create .draft/, the initial stable base,
                                 # and stable_head
draft checkpoint <name>          # capture a base state
# ... edit files ...
draft create <name>              # capture the change as a ChangePack
draft verify <pck_id>            # risk + evidence-based verification
draft review / draft approve     # review gates
draft submit                     # verified finalization: stable_head advances,
                                 # then the pack is disposed
```

With the default `merge_and_dispose` mode, each successful submit produces a new verified stable base. `draft event` and `draft receipt list` show the compact, tamper-evident history; full pack payloads are not retained.

### Recovering State

- `draft rollback chk_<id>` restores a checkpoint.
- `draft rollback rcp_<id>` restores the verified stable state a receipt anchors.
- `draft rollback pck_<id>` works only while the pack is active; after disposal Draft points you to the receipt instead.

### Maintenance And Exit

- `draft gc` prunes disposed or orphaned metadata, rebuilds indexes, and validates `stable_head`.
- `draft close` removes `.draft/` without touching project files; it refuses if unsafe pending state exists (`--force` overrides after a clear warning).

Draft-only workflows are fully offline: no remote server or registry is contacted. See [Protocol Contracts](../internals/protocol.md).
