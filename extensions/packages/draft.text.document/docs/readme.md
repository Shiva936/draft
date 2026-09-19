# Text documents

Makes line-level review possible.

Draft has no notion of a line. It compares authoritative state and knows _that_ a resource changed; explaining _how_ is a contributed capability, and this is the package that contributes it for text.

## What it contributes

- **`draft.text.document/document`** — the class assigned to text resources. A source file carries this _and_ its language class; neither displaces the other.
- **A comparison** — configures Draft's `sequence_alignment` engine to split on `0x0A` and emit regions in the `draft.text.document/line` coordinate space. The engine is Draft's; the claim that these tokens are lines is this package's.
- **Presentation bindings** — a text viewer for the resource, and a unit view for the change.

## Why it matters for composition

Without a comparison installed, two changes to one resource cannot be shown to be separable, so Draft refuses to compose them. With this package installed, edits to different line regions are provably independent and compose normally. That is the whole difference, and it is entirely a matter of what is installed.
