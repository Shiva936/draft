# Security Model

Draft is a local tool. Its security model focuses on protecting the local workspace, preserving signed audit evidence, and preventing Draft metadata from entering user change candidates.

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

`.draft/` is private metadata. Draft excludes it from status, scans, snapshots, Packs, submit candidates, rollback plans, watcher paths, and hook candidate checks.

If `.draft/` appears in a submit candidate, Draft:

1. warns;
2. aborts the submit;
3. emits `submit.completed` with failure status;
4. records a failed submit receipt;
5. skips `hooks.submit`.

## Event Integrity

Events are hash-chained and linked to signed receipts and the local transparency chain. Run:

```bash
draft doctor
draft receipt verify --all
```

This detects edits, missing links, parse failures, bad receipt signatures, and transparency-chain tampering. Event hashing and signed receipts are tamper-evident, not a substitute for backups.

## Security Identity And User Profile

The stable actor ID, Ed25519 signing/private key, published public-key records, trust state, authorization, ownership, candidate attribution, receipt identity, and canonical hashes are security/provenance state. They cannot be edited through profile configuration.

`user.name` and `user.email` are strictly non-security display/contact metadata. Changing them may alter selected config-layer bytes, the effective resolved profile, newly rendered non-authoritative display snapshots, and newly appended redacted config/profile audit events only. All pre-existing actor/key/ trust state, historical events and hashes, historical receipts and signatures, candidate attribution, workspace/source/Pack digests, and authorization or ownership results remain byte-for-byte or semantically identical and continue to verify.

Retired pre-release profile state is rejected without consuming its values. This includes `.draft/identity.json`, retired XDG profile files, `[identity]`, `identity.*`, retired profile environment variables, and former combined actor/profile fields. Diagnostics and `draft close` may identify the condition, guide recovery, or remove an unsupported workspace, but cannot treat the data as valid configuration.

## Import Boundary

`.draftpack` import is the untrusted-input boundary. Every archive is validated fail-closed before a byte reaches the quarantine: path traversal, absolute/UNC paths, `.draft/` writes, symlinks, hardlinks, device/fifo entries, invalid UTF-8 names, oversized artifacts, zip-bomb archives, corrupt or wrong-schema manifests and receipts, changes-hash mismatches, and embedded content objects whose bytes do not match their content address are all rejected. Imported packs enter `imports/quarantine/`, lose all origin trust marks, and must be locally re-verified and approved before they can be submitted. Submitting an imported pack re-checks integrity, applies the embedded content only if every touched file matches the change's recorded base version (nothing is written on any conflict), and checkpoints the workspace first so the apply is rollback-safe.

## Submit Gate

Submit is blocked unless the pack is verified and approved, the canonical risk report exists and reports no unresolved critical risk, the workspace hash still matches the verification state, and the event, receipt, and transparency chains verify. These gates are governed by the resolved policy (see [Review, Verification, And Policy](../reference/review-and-policy.md#policy)); the defaults fail closed. Policy resolves field by field from project policy, through global defaults, to Draft's built-in safe defaults.

## Hooks And External Commands

`hooks.submit`, `hooks.verify`, future `hooks.*` entries, and `spawn` execute local commands. Draft captures stdout, stderr, exit code, working directory, and command hash, but it does not sandbox the command. Users should configure commands carefully and review receipts.

## Rollback Safety

Rollback receipts and events should be reviewed after applying. Draft filters `.draft/` from rollback paths and rejects paths that escape the workspace root.

## Disclosure

Report security issues using the process in the root [SECURITY.md](../../SECURITY.md).

## Non-Goals

Draft does not provide hosted collaboration, pull requests, native merge behavior, remote synchronization, deployment, marketplace behavior, or credential exchange.
