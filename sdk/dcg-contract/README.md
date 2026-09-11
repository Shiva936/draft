# draft-dcg-contract

The portable Draft Change Graph (DCG) contract.

This crate is the canonical representation of every DCG value whose bytes take
part in a portable root, a portable interchange proof, or an independently
verifiable historical fact. It exists so that somebody who holds only a Draft
export — and none of Draft — can still check it.

## What you can do with only this crate

- Parse a `BaselineManifest`, recompute its `ProjectStateRoot`,
  `StateEvidenceRoot` and `CoverageEvidenceRoot`, and derive its `BaselineId`.
- Verify that an accepted state's evidence is exactly the evidence it claims:
  the state/evidence bijection, one primary observation per resource, and the
  disjointness of primary and corroborating observations.
- Walk a publication's chain — `PublicationRef` → `PublicationAttemptRef` →
  `PublicationOutcomeDigest` → `PublicationResolutionDigest` — and check each
  object's internal consistency, not merely that its digest matches its bytes.
- Check a receipt's structure and its Ed25519 signature, which covers the
  payload **and** the signer binding together.

## What this crate deliberately cannot do

It owns canonical *values*, never runtime behaviour. There is no storage, no
locking, no policy, no trust evaluation, no provider execution and no signing —
a crate that only checks signatures should not be able to produce them.

Several types here name concepts Draft implements elsewhere. A `LeaseFence` is
the number a lease held when an attempt was dispatched; lease lifetime and
enforcement live in Draft. A `ProjectSecurityStateDigest` is a digest; the state
it names, and every rule about mutating it, lives in Draft. Moving a value into
this crate does not move its subsystem.

That boundary is enforced mechanically rather than by convention:
`scripts/check-portable-contract-closure.sh` builds a crate outside the Draft
workspace that depends on this one alone, constructs every canonical type with
non-empty nested values, and fails if the resolved dependency graph reaches any
Draft crate.

## Verification levels

Receipt verification is reported in three separate levels, and they are never
collapsed into one word:

1. **Structural / canonical** — the document parses and is canonical.
2. **Cryptographic** — the signature verifies under the declared key.
3. **Trust and policy** — was that key trusted then, and is it trusted now?

This crate answers 1 and 2. It cannot answer 3, and a verifier built on it must
report level 3 as `unknown` rather than `valid`.

## Stability

`DCG_FORMAT_REVISION` is `1`. Every digest is taken under a frozen,
domain-separated, length-framed construction, and those separators are never
changed — changing one would silently change the identity of every historical
fact that used it.

Licensed under Apache-2.0.
