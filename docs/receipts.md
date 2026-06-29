# Receipts

A receipt is a durable record of an important action (especially finalization) under `.draft/receipts/` with a rebuildable `index.json`.

## Contents
Receipt id, Draft version, workspace + provider ids, actor, operation ids, Draft change ids, **provider object refs** (e.g. the Git commit SHA), risk + verification + finalization summaries, checkpoint refs, an undo hint, timestamp.

## Mapping
Receipts are how Draft maps neutral Draft changes to provider-native objects (`DraftChangeId → ProviderObjectRef`). This is the audit trail tying "what Draft reviewed" to "what the provider created".

## Commands
```
draft receipt list [--json]
draft receipt show <receipt-id> [--json]
```
Receipts never store raw secret values found by the risk engine.
