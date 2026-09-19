# Providers

A binding is a mutable pointer to an immutable semantic definition and operational profile. Unbind stops new routing; rebind restores it; neither rewrites history. Never put a secret in these files.

## Commands

- `draft project provider bind|list|show`
- `draft project provider unbind|rebind`

## Run

```sh
sh examples/providers/run.sh
```

Provider-gated: set `DRAFT_EXAMPLE_PROVIDER_DIR` to a directory holding `semantics.json`, `definition.json` and `profile.json` (canonical JSON). Without it the script prints `SKIPPED` and exits 0. `DRAFT_BIN` selects the binary (default: `draft` on `PATH`).
