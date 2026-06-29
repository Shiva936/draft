# Storage Layout

Draft stores all private state below `.draft/`. The store is native to Draft v0.3.0 and is independent of external tools.

## Top-Level Files

- `.draft/workspace.json`: workspace id, schema version, Draft version, and creation time.
- `.draft/config.toml`: workspace configuration.
- `.draft/.ignore`: Draft ignore rules.
- `.draft/verify.toml`: verification command configuration.
- `.draft/policy.toml`: save, review, and verification policy.

## Durable Directories

- `.draft/events/`: append-only hash-chained event log.
- `.draft/objects/`: content-addressed blobs for file contents, stdout, stderr, messages, and evidence.
- `.draft/snapshots/`: workspace manifests created by checkpoints and rollback-sensitive operations.
- `.draft/tasks/`: local task records.
- `.draft/runs/`: opaque command run records.
- `.draft/changepacks/`: changepack manifests, patches, reviews, and linked metadata.
- `.draft/evidence/`: verification and run evidence.
- `.draft/receipts/`: durable action receipts.
- `.draft/indexes/`: rebuildable SQLite indexes.
- `.draft/locks/`: local writer locks.
- `.draft/tmp/`: temporary files for atomic writes.

## Authority And Caches

The JSON and JSONL store is authoritative. The SQLite database is an index cache and can be rebuilt with:

```bash
draft index rebuild
```

Losing the index should not lose Draft history. Losing objects or JSON records may corrupt snapshots, evidence, or receipts.

## Event Log

Events are stored as JSON Lines. Each event includes:

- event id;
- event type;
- timestamp;
- actor;
- workspace id;
- optional subject id;
- payload;
- previous event hash;
- current event hash;
- schema version.

The append path uses a local writer lock and syncs the file after append. `draft events --verify-chain` verifies the chain.

## Object Store

Objects are addressed by SHA-256. Callers store bytes and receive a hash reference. The object store is used for workspace file contents, command output, rendered messages, and evidence payloads.

## Privacy

Draft can store file contents and command output. Treat `.draft/` as sensitive project metadata. Do not publish it unless the project intentionally wants to publish review history, evidence, and receipts.
