# Rollback

Rollback restores workspace files toward a prior Draft checkpoint, ChangePack boundary, or reversible receipt.

```bash
draft rollback <chk-id|pck-id|rcp-id>
```

Draft infers the rollback target type from the ID prefix. `chk_` rolls back to a checkpoint snapshot, `pck_` rolls back to the ChangePack boundary, and `rcp_` rolls back only when the receipt is reversible. Non-reversible receipts fail clearly.

## Safety Rules

Rollback must:

- never restore `.draft/`;
- reject paths that escape the workspace root;
- record a rollback event;
- write a receipt;
- make destructive behavior visible in the receipt and events.

## Operational Guidance

Create a checkpoint before risky work. If an agent run produces an unwanted result, inspect status and rollback with the relevant `chk_`, `pck_`, or reversible `rcp_` ID.
