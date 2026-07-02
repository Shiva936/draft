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

`draft task spawn` records task intent, candidate links, optional ChangePack links, and instruction text. This gives reviewers context for what the agent was asked to do before changes are packaged and reviewed.

## Review Checklist For Agent Changes

- Inspect every changed file.
- Read captured stdout and stderr when available.
- Run verification locally.
- Check risk findings for broad, binary, deletion, or sensitive-path changes.
- Require human approval for high-risk changes.
- Save only after policy is satisfied.

## Failed Runs

A failed agent command can still produce useful evidence. Keep the run record, inspect the workspace, and decide whether to create a ChangePack or roll back.
