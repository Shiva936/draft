# Security Policy

Draft is a local-first tool. Its main security boundary is protecting the workspace from unsafe Draft metadata inclusion, unsafe rollback paths, corrupted event history, and unreviewed external command execution.

## Supported Versions

The active development line is v0.3.x until the next minor line is released.

## Reporting

Please report suspected vulnerabilities privately to the project maintainers before public disclosure. Include the Draft version or commit tested, operating system and shell, reproduction steps, and whether `.draft/` contents, receipts, rollback, event integrity, or hooks were involved.

## Security-Sensitive Areas

- path normalization and traversal checks;
- symlink handling during scan and rollback;
- `.draft/` hard exclusion;
- event hash-chain verification;
- receipt integrity;
- external command capture through hooks;
- daemon IPC request validation.

## Non-Goals

Draft does not implement network publishing, hosted review flows, or credential exchange. Any external command is user-configured and receipt-backed.
