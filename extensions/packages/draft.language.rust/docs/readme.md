# Rust

Contributes the Rust domain to Draft.

Draft itself knows nothing about Rust: it manages resources, changes,
evidence and approvals. This package supplies the interpretation — which
resources are Rust sources, how to check them, and how to present
them — and Draft applies it without ever learning the language.

## What it contributes

- **`draft.language.rust/source`** — the class assigned to resources this package recognizes. A
  resource may carry other classes at the same time; classification composes.
- **Verification checks** — run through Draft's one authorized execution boundary, and only when the artifact holds a `process.execute` grant.
- **Risk rules** — weighted conditions over neutral facts. Draft evaluates them; it attaches no meaning to what they match.
- **A presentation binding** — selects a built-in viewer. It affects display
  only, and never verification, risk, conflict or submission semantics.

## Marker files

`Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`

These are conventional locations for this ecosystem. They are documentation
here, not behaviour: nothing in Draft reads this list.
