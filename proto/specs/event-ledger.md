# Activity Ledger Protocol

<!-- retired-architecture-ok: naming what was retired is the point. -->
`events/events.log` is the sole authoritative Activity file for one project, and
`events/events.index` is derived from it and fully rebuildable. There is no
`events.jsonl`, no alias, and no compatibility reader.

## Logical record versus physical frame

A logical `LedgerRecord` contains `event_id`, `previous_hash`, `record_hash` and
`payload`. The chain hash and the payload identity are computed over the
canonical logical record alone.

A record is stored inside a physical frame — magic, format marker, encoded
record length, canonical record bytes, checksum, terminator. The framing bytes
are **not** part of logical event identity. The frame exists only to classify a
damaged tail.

## Recovery

- A **physically incomplete** final frame — partial header, declared length past
  end-of-file, truncated record bytes or checksum, missing framing bytes — is
  truncated to the end of the last fully verified record, the index is rebuilt,
  and undrained audit facts replay through the normal idempotent append.
- A **physically complete but invalid** final frame, or corruption inside any
  earlier complete record, is never truncated and never replayed over. It routes
  to Doctor and recovery.

## Append

```
append(event_id, payload):
    acquire events/events.lock via ProcessFileLock   (internal, not a product lease)
    read the authoritative tail
    id present, payload identical  -> idempotent success, no second record
    id present, payload differs    -> corruption / conflict error
    id absent                      -> link LedgerRecord(previous_hash = tail); frame; fsync; index
    release
```

<!-- retired-architecture-ok: naming the rejected event name is the point. -->
The event vocabulary is closed, and every entry maps to a named audit fact and a
named journal mechanism. There is deliberately no `PublicationAttempted`.

`draft activity list --page --limit` is the query interface, `draft activity
show` reads one record, and `draft activity verify` verifies the chain. Draft
v0.3.4 defines no `draft log` command.
