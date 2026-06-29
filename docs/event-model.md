# Event Model

Draft records important state transitions as append-only events. Events are for auditability, debugging, service updates, and tamper evidence.

## Event Shape

Each event has:

- `id`: unique event id;
- `type`: event name;
- `time`: UTC timestamp;
- `actor`: resolved local actor;
- `workspace_id`: workspace identifier;
- `subject_id`: optional related object id;
- `payload`: event-specific JSON;
- `prev_event_hash`: previous event hash or null;
- `event_hash`: hash of the event envelope with this field empty;
- `schema_version`: store schema version.

## Hash Chain

The chain is linear. Every appended event points at the previous event hash. Verification fails if:

- an event cannot be parsed;
- a previous hash does not match;
- an event hash does not match its serialized envelope;
- an event was edited after append.

Run:

```bash
draft events --verify-chain
```

The core API and daemon also expose event replay summaries, which count events
by type after verifying the chain.

## Durability

Event append uses a local lock under `.draft/locks/`, appends one JSON object per line, and syncs the file. This keeps concurrent local writers serialized and makes abrupt process exits easier to diagnose.

## Event Types

Representative event types include:

- `WorkspaceInitialized`
- `WorkspaceScanned`
- `SnapshotCreated`
- `TaskCreated`
- `RunStarted`
- `RunCompleted`
- `ChangepackCreated`
- `VerificationCompleted`
- `RiskAssessed`
- `ReviewCommented`
- `ChangepackApproved`
- `ChangepackRejected`
- `ChangepackComposed`
- `SaveStarted`
- `SaveCompleted`
- `SaveFailed`
- `RollbackCreated`
- `RollbackApplied`

Consumers should tolerate new event types and additional payload fields.
