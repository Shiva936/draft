# draft-draftpack-contract

The portable DraftPack interchange format.

A DraftPack carries an accepted Baseline — its state, the provenance that
establishes it, its coverage evidence and its receipts — out of one Draft
installation so another can **verify** it.

This crate is the format contract, and nothing else. It has no tar reader, no
filesystem access, no extraction and no import policy; those live in Draft.

## Why the grammar is portable

Import is a security boundary: every archive is untrusted input. Keeping the
safe-path grammar and the format limits in a portable crate means an exporter
validates against exactly the rules an importer will apply — so an archive
cannot be produced that the recipient is obliged to refuse.

The grammar refuses absolute paths and drive prefixes, any `..` component,
writes into `.draft/`, backslashes (a separator on one platform and a filename
character on another), control characters, empty components, and over-long
names. Deserializing a path is a construction, so untrusted JSON cannot smuggle
one past the check.

## What a recipient can verify

1. The envelope's Ed25519 signature over the manifest **and** the signer
   binding together, so a pack cannot be re-attributed to another exporter.
2. Every member's bytes against the manifest's per-entry digests. Because the
   entry list is part of the signed manifest, a member that was added or removed
   is detected, not just one that was altered.
3. The Baseline's roots, recomputed from the carried DCG facts using
   `draft-dcg-contract` alone.

## What none of that establishes

That the pack should be **trusted**. A valid signature says these bytes came
from the holder of that key, unmodified. Whether that key is trusted here, and
whether the Baseline should be adopted, are the importer's decisions — and this
crate deliberately cannot make them.

`DRAFTPACK_FORMAT_REVISION` is `1`.

Licensed under Apache-2.0.
