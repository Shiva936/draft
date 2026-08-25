# Storage And Events

Draft stores private state in two hidden `.draft/` stores. The global store (`~/.draft/`) holds the stable security actor, signing keys, trust data, global configuration, adapter config, and reusable cache state. The project store (`<workspace>/.draft/`) holds workspace packs, events, receipts, transparency data, checkpoints, evidence, and indexes. Both stores are native to Draft v0.3.4 and independent of external tools.

## Global Store

- `~/.draft/config.toml`: global defaults, including optional `user.*` display/contact metadata.
- `~/.draft/identity/`: stable security actor and candidate registry; profile fields are forbidden here.
- `~/.draft/keys/signing.key`: private Ed25519 signing key.
- `~/.draft/trust/`: trusted and revoked public-key metadata.
- `~/.draft/adapters/`: local adapter configuration.
- `~/.draft/cache/`: rebuildable global caches.

The global store never stores project pack data.

## Project Top-Level Files

- `.draft/workspace.json`: workspace id, its contract schema version, product/provenance `draft_version`, and creation time. `draft_version` does not determine workspace compatibility.
- `.draft/config.toml`: workspace configuration.
- `.draft/.ignore`: Draft ignore rules.
- `.draft/verify.toml`: verification command configuration.
- `.draft/policy.toml`: submit, review, and verification policy.

## Durable Directories

- `.draft/events/`: append-only hash-chained event stream.
- `.draft/objects/`: content-addressed blobs for file contents, stdout, stderr, messages, and evidence.
- `.draft/snapshots/`: workspace manifests created by checkpoints and rollback-sensitive operations.
- `.draft/tasks/`: local task records.
- `.draft/executions/`: canonical task execution records.
- `.draft/pack-workspaces/`: mutable staging state while deriving immutable revisions.
- `.draft/evidence/`: verification and run evidence.
- `.draft/receipts/`: durable action receipts.
- `.draft/transparency/`: local tamper-evident receipt/event chain.
- `.draft/packs/`: canonical immutable pack manifests, revisions, lockfiles, and evidence summaries.
- `.draft/imports/quarantine/`: untrusted imported `.draftpack` artifacts, including their embedded content objects (`objects/<hash>`). A quarantined pack's whole directory moves to `.draft/packs/` when the import is submitted.
- `.draft/exports/`: local export outputs when requested.
- `.draft/lsif/`: basic offline symbol index.
- `.draft/indexes/`: rebuildable SQLite indexes.
- `.draft/locks/`: local writer locks.
- `.draft/tmp/`: temporary files for atomic writes.

The project store never stores the private signing key.

## Contract And Container Boundaries

Every independently decoded persisted or transmitted Draft contract owns a closed `ContractId`, stable unversioned identifier, and independent current and supported schema-version policy. Every policy currently supports only version `1`. Artifacts remain self-describing through their own `schema_version`; registry metadata validates the expected Rust contract and never reinterprets existing bytes.

A canonical SQLite database, archive, file, envelope, or other independently decoded container owns the version for that boundary. Rows or subrecords that are never encoded outside it inherit the container version. A member that is independently hash-addressed, signed, copied, persisted, transmitted, or decoded is a separate registered boundary with its own metadata. New contracts require a new compile-time `ContractId`; arbitrary runtime registration and string-selected production dispatch are not supported.

## Authority And Caches

The JSON and JSONL store is authoritative. The SQLite database is an index cache and is treated as rebuildable implementation state. Use `draft storage doctor` to inspect storage health.

Losing the index should not lose Draft history. Losing objects or JSON records may corrupt snapshots, evidence, or receipts.

## Event Log

Draft records important state transitions as append-only events for auditability, debugging, service updates, replay, tooling, and tamper evidence. Only the raw stream is stored; `draft event` derives a readable timeline from it.

### Event Shape

Each event includes:

- `event_id`: unique event ID;
- `type`: event name;
- `time`: UTC timestamp;
- `actor_id`: resolved local actor;
- `candidate_id`: optional candidate ID;
- `workspace_id`: workspace identifier;
- `subject_id`: optional related object ID;
- `previous_event_hash`: prior event hash or the genesis hash;
- `receipt_id`: linked receipt ID for trust-ledger events;
- `metadata`: event-specific JSON;
- `event_hash`: hash of the canonical envelope excluding this field.

### Hash Chain And Durability

The chain is linear: every appended event points to the prior hash. Verification fails when an event cannot be parsed, a previous hash does not match, the envelope hash is wrong, or a stored event was edited.

```bash
draft doctor
draft receipt verify --all
```

Canonical append writes one JSON object per line to `.draft/events/event.log`, holds a local writer lock, and syncs the file. Receipt-producing trust events preallocate their `rcp_` ID so the appended line is final when written. Core and daemon APIs can expose replay summaries that count events by type after verifying the chain.

### Event Types

Representative types include:

- `repo.initialized`, `workspace.scanned`, and `checkpoint.created`;
- `task.created`, `task.spawned`, `task.started`, and `task.completed`;
- `pack.created`, `pack.selected`, and `pack.deleted`;
- `verify.started`, `verify.completed`, and `risk.completed`;
- `review.started`, `review.completed`, `pack.approved`, and `pack.rejected`;
- `compare.completed`, `compose.completed`, and `disperse.completed`;
- `submit.started`, `submit.completed`, `rollback.started`, and `rollback.completed`;
- `storage.compacted` and `storage.gc_completed`.

Consumers must tolerate new event types and additional payload fields.

## Object Store

Objects are addressed by BLAKE3 (`b3:<hash>`). Object bytes are stored compressed with zstd, and `draft storage compact` can move loose compressed objects into zstd-compressed pack files with a rebuildable index. The object store is used for workspace file contents, command output, rendered messages, and evidence payloads.

## Privacy

Draft can store file contents and command output. Treat `.draft/` as sensitive project metadata. Do not publish it unless the project intentionally wants to publish review history, evidence, and receipts.
