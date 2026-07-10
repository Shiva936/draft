# Security Model

Draft v0.3.3 is a local tool. Its security model focuses on protecting the local workspace, preserving signed audit evidence, and preventing Draft metadata from entering user change candidates.

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

`.draft/` is private metadata. Draft excludes it from scans, snapshots, ChangePacks, save candidates, rollback plans, and hook candidate checks.

If `.draft/` appears in a save candidate, Draft:

1. warns;
2. aborts the save;
3. emits `save.completed` with failure status;
4. records a failed save receipt;
5. skips `hooks.save`.

## Event Integrity

Events are hash-chained and linked to signed receipts and the local transparency chain. Run:

```bash
draft doctor
draft receipt verify --all
```

This detects edits, missing links, parse failures, bad receipt signatures, and transparency-chain tampering. Event hashing and signed receipts are tamper-evident, not a substitute for backups.

## Import Boundary

`.draftpack` import is the untrusted-input boundary. Every archive is
validated fail-closed before a byte reaches the quarantine: path traversal,
absolute/UNC paths, `.draft/` writes, symlinks, hardlinks, device/fifo
entries, invalid UTF-8 names, oversized artifacts, zip-bomb archives, corrupt
or wrong-schema manifests and receipts, changes-hash mismatches, and embedded
content objects whose bytes do not match their content address are all
rejected. Imported packs enter `imports/quarantine/`, lose all origin trust
marks, and must be locally re-verified and approved before they can be saved.
Saving an imported pack re-checks integrity, applies the embedded content only
if every touched file matches the change's recorded base version (nothing is
written on any conflict), and checkpoints the workspace first so the apply is
rollback-safe.

## Save Gate

Save is blocked unless the pack is verified and approved, the canonical risk
report exists and reports no unresolved critical risk, the workspace hash
still matches the verification state, and the event, receipt, and
transparency chains verify. These gates are governed by the resolved policy
(see [policy.md](policy.md)); the defaults fail closed.

## Hooks And External Commands

`hooks.save`, `hooks.verify`, future `hooks.*` entries, and `spawn` execute local commands. Draft captures stdout, stderr, exit code, working directory, and command hash, but it does not sandbox the command. Users should configure commands carefully and review receipts.

## Rollback Safety

Rollback receipts and events should be reviewed after applying. Draft filters `.draft/` from rollback paths and rejects paths that escape the workspace root.

## Disclosure

Report security issues using the process in the root [SECURITY.md](../SECURITY.md).
