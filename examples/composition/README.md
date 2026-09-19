# Composition

Three ChangePacks against one Baseline. Disjoint work is independent; work over the same Resource conflicts and names it. A set composes only if every pair is independent — composition refuses to guess.

## Commands

- `draft pack compare`, `draft pack conflicts`, `draft pack depends`
- `draft pack compose`, `draft pack disperse`

## Run

```sh
sh examples/composition/run.sh
```

Runs anywhere with only the `draft` binary, in a temporary project with its own global store. `DRAFT_BIN` selects the binary (default: `draft` on `PATH`).
