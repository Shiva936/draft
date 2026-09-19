# End to end

The complete local lifecycle in five milestones: open and seal, govern, promote to a Baseline, read Activity and the Promotion receipt, then start the Console and stop it cleanly. The Console step is bounded by its readiness line (`Open this URL in your browser: …`), confirms the origin answers when `curl` is available, sends Ctrl-C and stops the daemon.

## Commands

- `draft project init`
- `draft pack new`, `draft pack revision seal`
- governance, `draft promote`
- `draft console web --no-open --port 0 --project <id>`
- `draft daemon stop`

## Run

```sh
sh examples/end-to-end/run.sh
```

Runs anywhere with only the `draft` binary, in a temporary project with its own global store. `DRAFT_BIN` selects the binary (default: `draft` on `PATH`).
