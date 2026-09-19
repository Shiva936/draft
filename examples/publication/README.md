# Publication

Baseline → Publication → provider. Publication is a separate, optional effect: a failed delivery leaves the Baseline exactly as accepted. Credentials, where a provider needs any, come from environment variables only.

## Commands

- `draft project provider bind`
- `draft authority grant --capability draft.publish/v1`
- `draft baseline publish run|list`, `draft baseline publications`

## Run

```sh
sh examples/publication/run.sh
```

Provider-gated: set `DRAFT_EXAMPLE_PROVIDER_DIR` to a directory holding `semantics.json`, `definition.json` and `profile.json` (canonical JSON). Without it the script prints `SKIPPED` and exits 0. `DRAFT_BIN` selects the binary (default: `draft` on `PATH`).
