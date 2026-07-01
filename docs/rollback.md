# Rollback

Rollback restores workspace files toward a prior Draft snapshot or receipt.

## Preview First

```bash
draft rollback <snapshot-or-receipt> --plan
```

The plan lists affected files and warnings. Review the plan before applying.

## Apply

```bash
draft rollback <snapshot-or-receipt> --yes
```

Applying rollback is explicit because it can overwrite workspace files.

## Safety Rules

Rollback must:

- never restore `.draft/`;
- reject paths that escape the workspace root;
- record a rollback event;
- write a receipt;
- make destructive behavior visible before apply.

## Operational Guidance

Create a checkpoint before risky work. If an agent run produces an unwanted result, inspect status, review the latest checkpoint, preview rollback, then apply only after confirming the affected files.
