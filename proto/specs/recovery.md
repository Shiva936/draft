# Recovery Protocol

`draft recover` accepts a checkpoint (`chk_<id>`), a recoverable staged Change
(`chg_<id>`), or the Activity event that recorded a checkpoint (`evt_<id>`).

A recovery target is an anchor Draft can prove: an Activity event is
hash-chained and verified, so it is the durable fact. A signed receipt is not
required and is no longer accepted — v1 receipts attest Promotions and
Publications, and requiring one here made a local checkpoint depend on a signing
identity it has no reason to need.

Once a Change's mutable staging is disposed, its immutable revision remains
authoritative but is not itself a recovery snapshot. Recover to a checkpoint, or
to the Activity event that recorded one.

`plan` resolves the target and reports what would be restored and what would be
**removed** without mutating anything; `run` performs it. Removals are surfaced
separately and by name, because recovery deletes. Recovery always protects
`.draft/`.

A recovery record states what was actually achieved rather than an
unconditional "completed": applying restoration successfully is not the same as
having proved the target state, and the two must not read alike.
