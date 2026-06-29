# Security & privacy

- **Local-first**: no network is required for core functionality; Draft uploads nothing (source, diffs, receipts, logs, identity) by default. No telemetry.
- **Secrets**: the risk engine flags secret-like patterns but never stores the raw value in findings or receipts. Verification logs may contain secrets — treat `.draft/verification/logs/` accordingly.
- **Verification commands** run local processes; they come only from explicit config or an explicit `draft verify "<cmd>"`, are shown before running, use structured execution (no shell), and support timeouts.
- **Provider commands** use structured argument arrays (no shell injection) and return structured errors; they are confined to provider modules.
- **IPC** is local-only: a Unix socket with user-only permissions, path validation, traversal rejection, and no arbitrary command execution endpoint.
- **Crash safety**: atomic writes mean a partial operation/receipt is never read as valid. Integrity hashes detect corrupt operation records.
- v0.2.0 does **not** sign operations cryptographically (the format is structured for future signing).
