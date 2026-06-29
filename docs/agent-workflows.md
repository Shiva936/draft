# Agent Workflows

Draft is designed to make AI-generated changes reviewable before save.

## Recommended Flow

```bash
draft checkpoint "before agent run"
draft spawn --name "agent edit" -- <agent command>
draft status
draft pack create --name "agent change" --from-working-tree
draft verify <pack-id>
draft risk <pack-id>
draft review <pack-id>
draft approve <pack-id> --reason "human reviewed"
draft save <pack-id>
```

## Why Spawn Through Draft

`draft spawn` captures command output, exit code, timing, and evidence references. This gives reviewers context for what the agent attempted and whether it completed cleanly.

## Review Checklist For Agent Changes

- Inspect every changed file.
- Read captured stdout and stderr when available.
- Run verification locally.
- Check risk findings for broad, binary, deletion, or sensitive-path changes.
- Require human approval for high-risk changes.
- Save only after policy is satisfied.

## Failed Runs

A failed agent command can still produce useful evidence. Keep the run record, inspect the workspace, and decide whether to create a changepack or roll back.
