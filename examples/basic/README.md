# Basic

Open a ChangePack, edit, and seal a RevisionPack. Sealing the same state twice converges on the same `rpk_` id, because a RevisionPack is derived from its ChangePack and the exact state root.

## Commands

- `draft init` — the first Baseline
- `draft pack new` — a ChangePack (`cpk_`): the governable work lineage
- `draft pack revision seal` — a RevisionPack (`rpk_`): one immutable, exact revision
- `draft pack show`, `draft pack revision list|show`

## Run

```sh
sh examples/basic/run.sh
```

Runs anywhere with only the `draft` binary, in a temporary project with its own global store. `DRAFT_BIN` selects the binary (default: `draft` on `PATH`).
