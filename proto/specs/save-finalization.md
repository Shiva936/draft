# Save Finalization Protocol

`draft save` acquires a lock, validates the accepted pack or composition, runs
before hooks, executes the configured save mode, verifies project state, writes
receipts/events, advances `stable_head`, runs after hooks, and disposes pack
metadata last.

Supported save modes are `merge_and_dispose` and `dispose_only`.
