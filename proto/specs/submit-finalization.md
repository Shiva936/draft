# Submit Finalization Protocol

`draft submit` acquires a lock, validates the accepted pack or composition, runs
before hooks, executes the configured submit mode, verifies project state, writes
receipts/events, advances `stable_head`, runs after hooks, and disposes pack
metadata last.

Supported submit modes are `merge_and_dispose` and `dispose_only`.
