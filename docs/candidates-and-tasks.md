# Candidates And Tasks

Candidates and tasks connect user intent, local execution profiles, and ChangePack provenance.

## Candidates

Candidates are named execution profiles. They do not represent people, permissions, or hosted roles.

```bash
draft candidate add cli-helper --kind command -- "cargo test"
draft candidate list
draft candidate show cli-helper
draft candidate packs -c cli-helper
```

If `draft task spawn` references a missing candidate, Draft auto-registers it so the task can record provenance.

## Tasks

```bash
draft task spawn "agent edit" -c cli-helper -- "update parser error handling"
draft task list
draft task
```

`draft task spawn` records task intent, candidate links, optional ChangePack links, and instruction text. The current implementation records the task and provenance boundary; it does not execute a hosted agent or remote worker.

## Relationship To ChangePacks

A task can be linked to ChangePacks through task spawn and later review. ChangePacks remain the saved review unit.

