# GC Protocol

`draft gc` acquires a maintenance lock, validates stable-head integrity, preserves canonical pack history and recoverable staging, prunes only rebuildable/orphaned/temporary metadata, rebuilds indexes, and records `GcStarted`, `GcCompleted`, or `GcFailed`.
