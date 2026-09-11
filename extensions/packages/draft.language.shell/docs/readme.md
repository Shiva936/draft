# Shell

Contributes the Shell domain to Draft.

Draft itself knows nothing about Shell: it manages resources, changes,
evidence and approvals. This package supplies the interpretation — which
resources are Shell sources, how to check them, and how to present
them — and Draft applies it without ever learning the language.

## What it contributes

- **`draft.language.shell/source`** — the class assigned to resources this package recognizes. A
  resource may carry other classes at the same time; classification composes.
- **Verification checks** — run through Draft's one authorized execution boundary, and only when the artifact holds a `process.execute` grant.

- **A presentation binding** — selects a built-in viewer. It affects display
  only, and never verification, risk, conflict or submission semantics.

## Marker files

`.shellcheckrc`

These are conventional locations for this ecosystem. They are documentation
here, not behaviour: nothing in Draft reads this list.
