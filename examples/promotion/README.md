# Promotion

An approved RevisionPack becomes accepted state only through Promotion, which creates a new Baseline, completes the ChangePack and issues the Promotion receipt.

## Commands

- `draft baseline show` before and after
- `draft promote <cpk> <rpk> --gate <gate> --decision <decision>`
- `draft baseline receipts`, `draft activity list`

## Run

```sh
sh examples/promotion/run.sh
```

Runs anywhere with only the `draft` binary, in a temporary project with its own global store. `DRAFT_BIN` selects the binary (default: `draft` on `PATH`).
