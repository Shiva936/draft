# Review

Evidence → representation → assessment → review → gate → decision, each an immutable fact bound to one exact RevisionPack. A Decision authorizes; it accepts nothing and issues no receipt — receipts attest a Promotion or a Publication.

## Commands

- `draft pack representation list|show`
- `draft pack assess`, `draft pack review`
- `draft pack gates evaluate|list`, `draft pack decide`
- `draft pack receipts` — empty, by design

## Run

```sh
sh examples/review/run.sh
```

Runs anywhere with only the `draft` binary, in a temporary project with its own global store. `DRAFT_BIN` selects the binary (default: `draft` on `PATH`).
