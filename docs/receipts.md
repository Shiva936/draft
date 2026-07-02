# Receipts

Receipts are durable records of important Draft actions.

## Receipt Uses

Draft writes receipts for:

- checkpoints;
- verification;
- composition;
- save success;
- save failure;
- rollback;
- other review-significant actions.

## Save Receipts

A save receipt records:

- receipt id;
- changepack id;
- actor;
- native save status;
- hook status;
- overall status;
- rendered message object reference;
- optional hook results;
- start and end times;
- failure reason when applicable;
- receipt hash.

If `hooks.save` runs, the receipt records the hook name, command hash, shell, working directory, exit code, timestamps, and stdout/stderr object references.

If the Draft-native save succeeds but a required hook fails, the receipt records `native_save_status = "saved"`, `hook_status = "failed"`, and `overall_status = "failed"`. With `continue_on_error = true`, Draft records `overall_status = "saved_with_hook_failure"`.

## Failed Save Receipts

Failed saves are first-class. If save is blocked by policy, verification, approval, or `.draft/` candidate safety, Draft records a failed receipt and appends `save.completed` with failure status.

## Inspecting Receipts

```bash
draft receipt list
draft receipt show <receipt-id>
```

Receipts are review artifacts. They should be preserved when sharing Draft state for audit.
