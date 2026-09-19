# Canonicalization Protocol

Canonical JSON uses sorted object keys, stable array order, compact separators, normalized workspace-relative paths, and SHA-256 hashes prefixed with `sha256:`.

Canonicalization applies to ChangePacks, receipts, Activity records, Baseline manifests and their roots, the Publication family, compositions, config, and verification keys.

Every independently canonicalized, signed, hash-addressed, copied, persisted, transmitted, or decoded member is a contract boundary and owns an independently registered schema policy. An internal row/subrecord inherits its containing container's version only when it is never independently encoded.

`user.name` and `user.email` never participate in security/canonical actor, signature, authorization, trust, attribution, ownership, receipt-verification, Activity-record-hash, workspace/source, or ChangePack digest inputs. A presentation may carry a new non-authoritative display snapshot beside a stable actor ID.
