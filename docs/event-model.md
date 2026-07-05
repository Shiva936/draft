# Event Model

Draft records important state transitions as append-only events. Events are for auditability, debugging, service updates, replay, tooling, and tamper evidence. Draft stores only the raw event stream; `draft event` renders a human-readable timeline from those stored records.

## Event Shape

Each event has:

- `event_id`: unique event id;
- `type`: event name;
- `time`: UTC timestamp;
- `actor_id`: resolved local actor;
- `candidate_id`: optional candidate id;
- `workspace_id`: workspace identifier;
- `subject_id`: optional related object id;
- `previous_event_hash`: previous event hash, or the genesis hash for the first event;
- `receipt_id`: linked receipt id for trust-ledger events;
- `metadata`: event-specific JSON;
- `event_hash`: hash of the canonical event envelope excluding only `event_hash`.

## Hash Chain

The chain is linear. Every appended event points at the previous event hash. Verification fails if:

- an event cannot be parsed;
- a previous hash does not match;
- an event hash does not match its serialized envelope;
- an event was edited after append.

Run:

```bash
draft doctor
draft receipt verify --all
```

The core API and daemon also expose event replay summaries, which count events
by type after verifying the chain.

## Durability

Canonical event append writes one JSON object per line to `.draft/events/event.log` and syncs the file. Receipt-producing trust events preallocate their `rcp_` id before append, so the event line is final when written.

## Event Types

Representative event types include:

- `repo.initialized`
- `workspace.scanned`
- `checkpoint.created`
- `task.created`
- `task.spawned`
- `task.started`
- `task.completed`
- `pack.created`
- `pack.selected`
- `pack.deleted`
- `verify.started`
- `verify.completed`
- `risk.completed`
- `review.started`
- `review.completed`
- `pack.approved`
- `pack.rejected`
- `compare.completed`
- `compose.completed`
- `disperse.completed`
- `save.started`
- `save.completed`
- `rollback.started`
- `rollback.completed`
- `storage.compacted`
- `storage.gc_completed`

Consumers should tolerate new event types and additional payload fields.
