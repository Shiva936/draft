# Tasks And Runs

Tasks and runs help connect user intent, agent execution, evidence, and ChangePacks.

## Tasks

A task is a local record with:

- id;
- title;
- optional description;
- actor;
- risk profile;
- optional linked issue text;
- status;
- creation time.

Create and inspect tasks with:

```bash
draft task spawn "refactor parser" -- "clean up parse errors"
draft task list
draft task
```

Tasks can be linked to ChangePacks when spawned with `-p <ChangePack>` or through later review context.

## Runs

A run records an opaque command execution. Runs are useful for agent sessions, local scripts, and verification-like commands that should produce review evidence.

Run records include:

- command;
- shell;
- working directory;
- status;
- timestamps;
- stdout and stderr object references;
- exit code;
- linked task or ChangePack when available.

## Spawn

```bash
draft task spawn "agent edit" -c <candidate-name> -- <instruction>
```

`draft task spawn` records task intent, candidate links, optional ChangePack links, and instruction text. It does not decide whether resulting workspace changes are acceptable; that decision happens through ChangePack review, verification, risk, policy, and approval.

## Relationship To ChangePacks

A run may produce file changes. A ChangePack captures those changes later. Keeping both lets reviewers see both the final delta and the process that produced it.
