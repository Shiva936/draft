# Import / Export Protocol

A `.draftpack` carries one **accepted Baseline** out of a Draft installation so
another can verify it — without trusting the sender. It is an uncompressed,
deterministically ordered tar archive whose signed manifest authenticates the
complete canonical member set by per-member digest. The same Baseline exported
by the same producer produces the same bytes.

The format is defined by `draft-draftpack-contract`, which is Core-free: a
recipient can verify an archive without running Draft.

## Members

```
draftpack.json               the DraftpackEnvelope: manifest + signer binding + signature
baseline/manifest.json       the Baseline manifest
baseline/record.json         the acceptance record that promoted it
baseline/composition.json    which provider established which part
baseline/lineage.json        the accepted Baselines it descends from, newest first
```

The composition travels because the evidence root is a **digest**: it proves the
entries did not change and cannot say what they were. Without it a recipient
could verify the Baseline and still not know which provider established any part
of it.

The manifest names `project`, `baseline`, every `entry` (path, size, digest),
any `receipts` travelling with the archive, `exported_by`, `exported_at` and the
`producer` identity. Receipts are embedded rather than referenced so a recipient
can check the attestations without contacting the exporter.

## Why per-entry digests

A single digest over the whole archive proves the bytes are unchanged in transit
and nothing else. Per-entry digests additionally say *which* member is wrong
when one is, and — because the entry list is part of the signed manifest — make
a member that was silently added or removed a manifest mismatch rather than an
unnoticed difference.

## Export and import are deliberately asymmetric

Export reads state Draft already accepted, so it writes. Import reads bytes from
anywhere, so it validates the archive completely before a byte reaches the
quarantine:

1. **Path safety.** Path traversal, absolute paths, `.draft/` writes, symlinks,
   hardlinks, device entries, invalid UTF-8 names, oversized artifacts and
   zip-bomb archives are rejected while the archive is being read.
2. **Manifest completeness.** Every member is checked against the manifest, and
   the manifest against every member. Missing, extra and corrupted members are
   reported by name.
3. **Signature.** Whether the exporter's signature covers exactly this manifest.

## Import grants nothing

Those three checks establish that the bytes are unchanged and that whoever held
the key produced them. They never establish that the key is trusted **here**;
that is the importer's question, and answering it from inside the archive would
let a sender vouch for itself.

So a verified archive lands in `imports/quarantine/<baseline>/`, and the report
states `locally_accepted: false` explicitly rather than by omission. An imported
Baseline becomes authoritative in this project only by going through this
project's own Promotion, like anything else.

Importing a second artifact claiming a Baseline already in quarantine is refused
rather than merged: two archives claiming one identity are two different claims,
and overwriting one with the other would destroy the evidence that they
disagreed.

Portable payloads are path-safe, deterministic, locally verifiable, and never
include `.draft/` metadata, signing keys or local trust decisions.
