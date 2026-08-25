# Submit Finalization Protocol

`draft submit` acquires a lock, validates the approved immutable revision, runs before hooks, executes the configured submit mode, verifies project state, writes receipts/events, advances `stable_head`, runs after hooks, and disposes mutable staging last. Canonical manifests, revisions, lifecycle records, evidence, events, and receipts are never disposed by submit.

Supported submit modes are `merge_and_dispose` and `dispose_only`.
