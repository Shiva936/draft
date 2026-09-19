# Multi-resource

One ChangePack whose scope names several Resources, one of them new. Shows the resolved scope, what the sealed RevisionPack touched, what it reaches and what evidence covers.

## Commands

- `draft pack scope`
- `draft pack impact` and `draft pack coverage` — nothing is inferred
- `draft pack inspect` — everything recorded about the ChangePack

## Run

```sh
sh examples/multi-resource/run.sh
```

Runs anywhere with only the `draft` binary, in a temporary project with its own global store. `DRAFT_BIN` selects the binary (default: `draft` on `PATH`).
