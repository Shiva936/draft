# Agent Workflows

Draft is designed to make AI-generated changes reviewable before save.

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
draft save -p <pck-id>
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
