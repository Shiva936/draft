# Release Notes

See [CHANGELOG.md](CHANGELOG.md) for the public changelog.

## v0.3.3

Draft v0.3.3 finalizes the local stable-base stability model. Draft defines
project stability through verified stable base states: `stable_head` points to
the latest verified stable base, and a changepack is temporary until
`draft save` verifies and finalizes it.

This release introduces the stable base and `stable_head` model,
project-state verification receipts (`ProjectStateVerified` /
`ProjectStateVerificationFailed`), a configurable `draft save` finalization
pipeline (`[save].pack_disposal = "merge_and_dispose" | "dispose_only"`),
phased save hooks (`before` runs pre-finalization, `after` runs post-advance
and pre-disposal), changepack disposal with compact provenance retention,
`draft close` and `draft gc`, composition validation for
independent/dependent/conflicting packs with deterministic composition
hashes, deterministic verification cache keys
(workspace + config + toolchain + command + environment hashes), the
top-level `proto/` protocol contract layer (specs, schemas, and test
vectors), human-readable CLI output by default, and non-destructive migration
from v0.3.2.

Pack validity is not project stability: `stable_head` advances only after the
composed final state passes project-level verification, and pack disposal is
always the final successful step — every failure path preserves recoverable
state. DraftHub, CI/CD, remote review, and environment heads remain deferred
to v0.4.0; v0.3.3 produces the deterministic receipts, hashes, and
`stable_head` metadata they will build on.

No `draft log`; use `draft event` with `--page`/`--limit`. IDs remain
`chk_`/`pck_`/`rcp_`.

### Safety

- `stable_head` advances only after successful project-state verification;
  failed saves preserve the changepack and record failure receipts/events.
- Pack disposal happens only after all configured save steps succeed; disposal
  failure records `PackDisposalFailed` and `draft gc` can retry cleanup.
- `.draft/` is hard-excluded from status, snapshots, changepacks, save
  candidates, rollback plans, workspace hashes, and external command candidate
  checks. If `.draft/` appears in a save candidate, Draft aborts the save.
- Hooks are opaque and untrusted: Draft captures stdout, stderr, exit code,
  command hash, and receipt linkage without interpreting the command, and a
  non-zero exit fails the save.
- `draft close` never deletes project files and refuses unsafe pending state
  without `--force`.
