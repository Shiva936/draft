# Recovery

Checkpoint, make a mess, then plan, preview and restore. Recovery restores a recorded state and proves it; it is not a rollback of accepted state — the accepted Baseline moves only by Promotion.

## Commands

- `draft pack checkpoint`
- `draft recover plan|dry-run|run`

## Run

```sh
sh examples/recovery/run.sh
```

Runs anywhere with only the `draft` binary, in a temporary project with its own global store. `DRAFT_BIN` selects the binary (default: `draft` on `PATH`).
