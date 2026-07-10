# GC Protocol

`draft gc` acquires a maintenance lock, validates stable-head integrity,
preserves active and recoverable packs, prunes safe disposed/orphaned/temp
metadata, rebuilds indexes, and records `GcStarted`, `GcCompleted`, or
`GcFailed`.
