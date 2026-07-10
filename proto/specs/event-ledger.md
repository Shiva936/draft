# Event Ledger Protocol

The event ledger is append-only or equivalently tamper-detectable. Each event
contains `event_id`, `event_type`, `timestamp`, optional `subject_id`, optional
`receipt_id`, `payload_hash`, `previous_event_hash`, and `event_hash`.

`draft event --page --limit` is the query interface. Draft v0.3.3 does not define
a `draft log` command.
