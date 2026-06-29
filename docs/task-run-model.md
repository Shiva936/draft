# Tasks And Runs

Tasks and runs help connect user intent, agent execution, evidence, and changepacks.

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
draft task create "refactor parser" --description "clean up parse errors"
draft task list
draft task show <task-id>
```

Tasks can be linked to changepacks when the change is created.

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
- linked task or changepack when available.

## Spawn

```bash
draft spawn --name "agent edit" -- <command>
```

`spawn` captures command evidence. It does not decide whether the resulting workspace changes are acceptable; that decision happens through changepack review, verification, risk, policy, and approval.

## Relationship To Changepacks

A run may produce file changes. A changepack captures those changes later. Keeping both lets reviewers see both the final delta and the process that produced it.
