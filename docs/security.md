# Security Model

Draft v0.3.0 is a local tool. Its security model focuses on protecting the local workspace, preserving audit evidence, and preventing Draft metadata from entering user change candidates.

## Trust Boundary

Trusted:

- the local user account running Draft;
- files the user chooses to scan and review;
- configured local commands after explicit approval.

Untrusted or sensitive:

- command output captured as evidence;
- generated changes from agents;
- paths from scripts;
- symlinks and unusual filesystem entries;
- corrupted or manually edited `.draft/` files.

## Hard `.draft/` Exclusion

`.draft/` is private metadata. Draft excludes it from scans, snapshots, changepacks, save candidates, rollback plans, and external command candidate checks.

If `.draft/` appears in a save candidate, Draft:

1. warns;
2. aborts the save;
3. emits `SaveFailed`;
4. records a failed save receipt;
5. skips `target.local`.

## Event Integrity

Events are hash-chained. Run:

```bash
draft events --verify-chain
```

This detects edits, missing links, and parse failures. Event hashing is tamper-evident, not a substitute for backups or cryptographic signing.

## External Commands

`target.local` and `spawn` execute local commands. Draft captures stdout, stderr, exit code, working directory, and command hash, but it does not sandbox the command. Users should configure commands carefully and review receipts.

## Rollback Safety

Rollback plans should be reviewed before applying. Draft filters `.draft/` from rollback targets and rejects paths that escape the workspace root.

## Disclosure

Report security issues using the process in the root [SECURITY.md](../SECURITY.md).
