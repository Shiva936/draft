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

`.draft/` is private metadata. Draft excludes it from status, scans, snapshots, ChangePacks, change candidates, recovery plans, watcher paths, and hook candidate checks.

If `.draft/` appears in a change candidate, Draft warns, refuses the operation, and records the refusal. It never partially applies work that reached into its own metadata.

## Activity Integrity

The Activity Ledger is hash-chained and framed, and receipts are entered in the local transparency chain. Run:

```bash
draft doctor
draft doctor receipts --all
```

This detects edited records, missing links, parse failures, bad receipt signatures, and transparency-chain tampering.

A physically incomplete final frame — a crash part-way through an append — is truncated back to the last fully verified record and safely replayed. A physically complete frame whose checksum, parse, chain hash or payload integrity fails is **never** truncated and never replayed over, and neither is corruption inside any earlier record: those route to Doctor and recovery, because automatically rewriting history is worse than refusing.

Hash chaining and signed receipts are tamper-evident. They are not a substitute for backups.

## Correctness locks

Draft's authoritative state is guarded by kernel-owned, non-inheritable `ProcessFileLock`s on stable sidecar paths. They are correctness locks, not advisory hints: there is no wall-clock takeover, so a live but slow holder can never have its lock stolen out from under a compare-exchange.

Handles are opened close-on-exec (and non-inheritable on Windows), so a spawned child never silently retains a lock its parent held. That matters because Draft runs configured commands and extension programs: a lock that survived into a child would outlive the operation that took it.

### One partial order

```
 1. TrustReadFence                    the global trust registry
 2. ProjectControlLease               product lease
 3. PublicationLease(pub_)            product lease
 4. ProjectControlStore               project/control.lock
 5. ProviderBindingStore              provider-bindings/<pbd_>.lock
 6. PublicationJournalStore           publication/journal/<pat_>.lock
 7. PublicationControlStore           publication/control/<pub_>.lock
 8. Per-record domain stores          changes/<cpk_>.lock, tasks/<tsk_>.lock
 9. Publication registry / outcome-head / resolution-head / retry-authorization
10. Activity Ledger                   events/events.lock
```

The rule is exactly one sentence:

> On acquiring lock _X_, every correctness lock **currently held** must have a lower order number than _X_.

It constrains **nested, simultaneously held** locks — not the chronological stream of acquisitions across a whole operation. So this is legal, because nothing higher is still held when the second phase begins:

```
acquire 6 → acquire 9 → release 9 → release 6 → acquire 6 → acquire 7 → …
```

and this is not:

```
hold 9 → acquire 7
```

At most one group-8 lock and at most one group-9 lock is held at a time. An operation acquires only the smallest set it actually needs; it never takes a lock merely because that lock sits earlier in the order. No reverse acquisition path exists anywhere, so no cycle is possible.

`scripts/check-lock-order.sh` asserts this statically and the runtime instrumentation asserts it dynamically. Neither checks chronological monotonic numbering — that would be a different, and wrong, check.

### A guarded read is not a held guard

Every authoritative journal read and every journal transition happens under the journal's guard. That is not the same as holding one guard across a whole multi-phase recovery workflow, and where a workflow would otherwise hold a journal (order 6) and reach for order 1, 3, 4 or 5, it **must** release and reacquire instead.

So committed-`AttemptPrepared` recovery reads the journal, releases it, reads the control record separately, releases that, classifies — and only then, in a new phase holding neither, takes the trust fence, the lease, the project control lock and the binding lock. It reacquires the journal last, re-reads the authoritative state, requires it still equals the exact expected value, and only then transitions.

**Every phase boundary re-reads before it mutates.** If the journal or the control record moved, the stale validation is discarded, the guards are released, and the workflow reclassifies from authoritative state. Three separate acquisitions of the same lock across one workflow is legal precisely because the rule is held-lock ordering rather than chronological monotonicity.

The one exception is pre-dispatch abandonment, which has exactly one frozen shape: the fresh-validation guards (1, 3, 4, 5) **remain held** through the abandonment commit (6, 7). AC never releases them and revalidates under a different security snapshot — the refusal snapshot recorded in `AbandonPrepared` is precisely the one validated while those exact guards were held.

### Journal serialization

Every per-record journal transition goes through its owning store's guarded `transition_locked`, and no journal record is ever written outside that store. There is no last-writer-wins path to a journal state.

**No lock — journal, provider binding, control or lease — is held across an external provider call.** The provider binding lock is held across exact route validation, attempt completion and the durable dispatch transition, and released _before_ the network call.

An audited mutation runs as: take the record lock → resolve any unresolved journal → re-read → check the caller's expected state → preallocate the event id → journal `Prepared` → write → journal `Committed` → release → drain the audit fact → journal `Finalized`. The drain happens outside the record lock, because the ledger is innermost in the order and holding a record across it would serialize unrelated work behind it. That is safe precisely because the drain is idempotent on the preallocated id.

## Security Identity And User Profile

The stable actor ID, Ed25519 signing/private key, published public-key records, trust state, authorization, ownership, candidate attribution, receipt identity, and canonical hashes are security/provenance state. They cannot be edited through profile configuration.

`user.name` and `user.email` are strictly non-security display/contact metadata. Changing them may alter selected config-layer bytes, the effective resolved profile, newly rendered non-authoritative display snapshots, and newly appended redacted config/profile audit events only. All pre-existing actor/key/ trust state, historical events and hashes, historical receipts and signatures, candidate attribution, workspace/source/ChangePack digests, and authorization or ownership results remain byte-for-byte or semantically identical and continue to verify.

Retired pre-release profile state is rejected without consuming its values. This includes `.draft/identity.json`, retired XDG profile files, `[identity]`, `identity.*`, retired profile environment variables, and former combined actor/profile fields. Diagnostics and `draft maintenance remove-project` may identify the condition, guide recovery, or remove an unsupported workspace, but cannot treat the data as valid configuration.

## Installation Boundary

`draft update` trusts only what the binary already trusts: `release-manifest.json` must carry a valid Ed25519 signature, over its exact published bytes, from a key embedded in this build, before it is parsed; every artifact must then match the manifest's digest and size before it is opened, and archive extraction refuses traversal, absolute paths, links, devices, duplicates, unexpected entries and anything past its size caps. The first-run installers are a different, weaker trust stage — HTTPS plus the published `SHA256SUMS` — and never claim to verify the signed manifest.

The installation's private lifecycle directory (`<install_root>/.draft-install/`, `0700` on Unix, inherited user ACLs on Windows) holds the receipt, the permanent lock and the operation journal. The identity digest in `bootstrap.recovery` is an integrity/consistency check inside that directory, not a signature: a same-user process able to rewrite both it and the helper is not defeated by it. Uninstall deletes only receipt-typed, identity-validated slots and never a project; `--purge` deletes the global store only after proving its `home.json` ownership marker.

## The promotion boundary

Promotion is the single point at which a project changes what it accepts, and it happens only on an approving Decision that cites a satisfied Gate over the **exact** revision being promoted. A decision about a different revision, a gate over a different revision, or a decision made against a Baseline the project has since moved past are each refused rather than reinterpreted — Draft does not rebase an authorization onto state nobody judged it against.

The commit itself is the locked compare-exchange of the project's control state, performed under a non-stealable trust fence with the ChangePack's lock held, so `Active → Completed` is deterministic finalization rather than a second, separately-failable step. Gates are governed by the resolved policy (see [Review, Verification, And Policy](../reference/review-and-policy.md#policy)); the defaults fail closed. Policy resolves field by field from project policy, through global defaults, to Draft's built-in safe defaults.

## Extension Capabilities

Four decisions are kept apart, and none of them implies the next:

```text
TRUST SOURCE  ≠  INSTALL PACKAGE  ≠  AUTHORIZE CAPABILITY  ≠  RUN A DECLARED COMMAND
```

An extension package is data. The most powerful thing it can declare is a program name, an argument vector, a workspace-relative working directory and a time limit — never a script, an entrypoint, or code Draft loads. Installing a package grants nothing: it is installed, enabled and serving its static contributions with any declared command inert.

Authorization is bound to one exact artifact: extension id, source, publisher, package version and content digest. A grant therefore never survives an update, including an update that requests exactly the permissions already approved, because a new version or new content is a different artifact. Superseded grants are retained rather than deleted, so the record of what was once permitted survives. Trusted provenance is not a permission: an officially published, signature-verified package holds nothing until the user says so.

Commands that are not authorized never reach Draft's domain logic at all. They are removed when contributions are resolved, so there is no path by which an unauthorized command could be run by mistake.

When Draft does run a declared command it runs it itself: the program is spawned directly with its argument vector, so there is no shell and therefore no quoting, globbing, redirection or command chaining available to a package. The environment is cleared apart from an explicit allowlist, the working directory is confined to the workspace, a time limit is enforced by killing the child, and captured output is bounded and redacted before it can reach durable evidence.

Signing keys for official catalogs are owned by an authorized signing environment. The packaging tool reads key material a caller supplies and never generates, stores or commits any, so ordinary package generation cannot create a root of trust by accident.

## Hooks And External Commands

Configured `hooks.*` entries and `draft task spawn` execute local commands. Draft captures stdout, stderr, exit code, working directory, and command hash, but it does not sandbox the command. Users should configure commands carefully and review receipts.

## Recovery safety

Recovery deletes, so a plan lists what would be **removed** separately and by name before anything runs, and the record afterwards states what was actually achieved rather than an unconditional "completed". Draft filters `.draft/` from recovery paths and rejects paths that escape the project root.

## Disclosure

Report security issues using the process in the root [SECURITY.md](../../SECURITY.md).

## Non-Goals

Draft does not provide hosted collaboration, pull requests, native merge behavior, remote synchronization, deployment, marketplace behavior, or credential exchange.
