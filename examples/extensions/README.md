# Extensions

Install a first-party package from the repository's `extensions/packages` and see its contributions on a Pack. Installing grants nothing: authorization is a separate, audited decision bound to one exact artifact.

## Commands

- `draft extension install|list`
- `draft extension tool list`
- `draft pack representation show`, `draft pack evidence run`

## Run

```sh
sh examples/extensions/run.sh
```

Extension-gated: set `DRAFT_EXAMPLE_EXTENSIONS` to the repository's `extensions/packages` directory. Without it the script prints `SKIPPED` and exits 0. `DRAFT_BIN` selects the binary (default: `draft` on `PATH`).
