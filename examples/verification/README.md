# Verification

Verification evidence about an exact RevisionPack. With nothing able to check it, evidence is honestly `unavailable`; after the project declares a check in `.draft/verify.toml` it is produced. A hook is an explicit local command whose run is recorded — never a Promotion.

## Commands

- `draft pack evidence run|list`
- `draft config hook set|run`

## Run

```sh
sh examples/verification/run.sh
```

Runs anywhere with only the `draft` binary, in a temporary project with its own global store. `DRAFT_BIN` selects the binary (default: `draft` on `PATH`).
