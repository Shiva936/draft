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
- status;
- rendered message object reference;
- optional `target.local` command hash;
- optional external command result;
- start and end times;
- failure reason when applicable;
- receipt hash.

If `target.local` runs, stdout and stderr are stored as objects and referenced by the receipt.

## Failed Save Receipts

Failed saves are first-class. If save is blocked by policy, verification, approval, or `.draft/` candidate safety, Draft records a failed receipt and appends `SaveFailed`.

## Inspecting Receipts

```bash
draft receipt list
draft receipt show <receipt-id>
```

Receipts are review artifacts. They should be preserved when sharing Draft state for audit.
