# Draft-Only Workflows

Draft works without Git or any other VCS. In a Draft-only repository, verified
stable base states are the project's record of permanence.

## The Loop

```text
draft init                      # create .draft/, the initial stable base,
                                # and stable_head
draft checkpoint <name>         # capture a base state
# ... edit files ...
draft create <name>             # capture the change as a changepack
draft verify <pck_id>           # risk + evidence-based verification
draft review / draft approve    # review gates
draft save                      # verified finalization: stable_head advances,
                                # the pack is disposed
```

With the default `merge_and_dispose` mode, each successful save produces a new
verified stable base. `draft event` and `draft receipt list` show the compact,
tamper-evident history; full pack payloads are not retained.

## Recovering State

- `draft rollback chk_<id>` restores a checkpoint.
- `draft rollback rcp_<id>` restores the verified stable state a receipt
  anchors.
- `draft rollback pck_<id>` works only while the pack is still active; after
  disposal Draft points you to the receipt instead.

## Maintenance And Exit

- `draft gc` prunes disposed/orphaned metadata, rebuilds indexes, and
  validates `stable_head`.
- `draft close` removes `.draft/` without touching project files; it refuses
  if unsafe pending state exists (`--force` overrides after a clear warning).

Draft-only workflows are fully offline: no remote server or registry is
contacted. See [Protocol Contracts](protocol.md) and
[Git Workflows](git-workflows.md).
