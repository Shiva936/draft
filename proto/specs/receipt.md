# Receipt Protocol

A v1 receipt attests exactly one of three things, named by `ReceiptKind`:

```
Promotion              { promotion, baseline }   a Promotion accepted a Baseline
PublicationOutcome     { outcome }               an attempt reached a primary outcome
PublicationResolution  { resolution }            an authorized interpretation became authoritative
```

Nothing else is receipted. A local action that is already an immutable fact in its own store — a checkpoint, a decision, a sealed revision — produces no receipt, because two records of one act with no rule for which is authoritative is worse than one.

Receipt ids use `rcp_<id>` and are **preallocated** by the transaction that will issue them. That is what makes finalization idempotent: replaying it writes the same receipt under the same id and converges, instead of minting a second receipt claiming to be the one.

## The envelope

```
ReceiptPayload         { receipt_id, subject, issued_by, issued_at }
ReceiptSignerBinding   { signer_identity, signing_key_id, signature_algorithm }
ReceiptSigningMessage  { payload, signer }        the ONLY thing signed
ReceiptEnvelope        { payload, signer, signature }
```

The `receipt_id` lives inside the payload, so the signature covers it: a signed payload cannot be re-filed under a different id. The signer binding is inside the signed message too — otherwise an attacker could keep a valid signature and rewrite who it claims to be from.

The signed bytes are the canonical `ReceiptSigningMessage` framed under a frozen domain separator, never "the serialized record". Field order, added optional fields and formatting would all change serialized bytes without changing meaning, so a signer and a verifier built at different times could disagree about what was signed while both behaving correctly.

Envelopes are stored create-once under their own `rcp_` id, bound to the digest of their canonical bytes. A second, different attestation under the same id is an integrity failure, not an overwrite.

## Verification reports three levels separately

```
Structure / canonical form   do the stored bytes re-derive the signed message?
Signature                    does it cover exactly this payload and binding?
Historical trust             was the key accepted when the receipt was issued?
Current trust                is the key accepted now?
```

Each answer is `valid`, `invalid`, or `unknown`. **What cannot be determined reads `unknown`, never `valid`** — an unresolvable key means Draft could not tell, which is a different answer from "the receipt failed".

"Signature valid, key since revoked" is a real state, and collapsing these into one yes/no is how it becomes invisible.
