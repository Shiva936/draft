# Command Reference

The Draft CLI is the primary v0.3.1 interface. Core workflows are local-first and work without a daemon.

Most read commands support `--json`. Human output is intended for terminals; JSON output is intended for scripts and TUI integration.

## Workspace

### `draft init [-b <base-pack-name>]`

Initializes `.draft/`, creates `.draft/events/events.jsonl`, writes default config files, creates the base pack, and selects it. The default base pack name is `base`.

### `draft status [-p <pck-id>] [-c repo|tasks|candidates|changes|hooks] [--full]`

Shows workspace or pack status. `.draft/` is always hard-excluded.

### `draft checkpoint <message>`

Creates a checkpoint with a `chk_` ID and a receipt.

## Config, Hooks, And Ignore Rules

### `draft config [-k <key>]`
### `draft config set <key> <value>`
### `draft config unset <key>`

Reads and writes effective config. Repo `.draft/config.toml` overrides global `~/.draft/config.toml`.

### `draft hook [-k <key>]`
### `draft hook set <key> <value>`
### `draft hook unset <key>`
### `draft hook run <hook-name>`

Manages hook configuration. `hooks.save` is the save hook; Draft has no native commit, push, pull, sync, PR, MR, publish, host-specific, or remote commands.

### `draft ignore add|remove|list`

Manages `.draft/.ignore`. `.draft/` remains hard-excluded even if ignore rules are changed.

## Events And Logs

### `draft log [--top|--bottom] [-p <page>] [-l <entries>] [-f <filter>] [--raw]`

Renders a timeline derived from `.draft/events/events.jsonl`. `--raw` prints the raw stream.

### `draft events`

Prints the raw append-only event stream.

## Packs

### `draft create <name> [-p <base-pck-id/name>]`
### `draft pack`
### `draft pack -s <pck-id/name>`
### `draft pack -d <pck-id/name>`
### `draft list`

Creates, shows, switches, deletes, and lists packs. Pack IDs use `pck_`. Pack names must be unique among available packs. `draft pack -d` asks for final `y/N` confirmation and emits `pack.deleted`.

## Candidates And Tasks

### `draft candidate list|show|remove`
### `draft candidate add <name> [--kind command|chat|manual] -- <template>`
### `draft candidate update <name> [--kind command|chat|manual] -- <template>`
### `draft candidate packs [-p <pck-id>] [-c <candidate-name>]`

Manages host-agnostic candidate execution profiles. Missing candidates referenced by task spawn are auto-registered.

### `draft task spawn "<name>" [-p <pck-id>] [-c <candidate-name> ...] [--cron <expr>] -- <instruction>`
### `draft task list`
### `draft task`

Spawns tasks and records task/candidate/pack provenance. `draft task` shows the latest task or reports that no task is running.

## Verify, Risk, Review, And Decisions

### `draft verify [-p <pck-id>]`

Verifies a pack, defaulting to the selected pack.

### `draft risk [-p <pck-id>] [--explain] [--include-evidence]`

Runs deterministic local risk analysis, defaulting to the selected pack.

### `draft review [-p <pck-id>] [--tui]`

Starts review and locks the pack for final human decision. `--tui` opens the Review Cockpit.

### `draft approve [-p <pck-id>]`
### `draft reject [-p <pck-id>]`

Records a mandatory human final decision. Review is required before approve/reject.

## Compare, Compose, Disperse, Save, Rollback

### `draft compare <pck-a> <pck-b> [--tui]`
### `draft compose <pck-a> <pck-b> --output <name> [--tui]`
### `draft disperse <pck-id> --output <pack-a-name> <pack-b-name> [--tui]`

Compares, combines, or splits packs with receipt-backed provenance.

### `draft save [-p <pck-id>] [--var key=value ...]`

Saves an approved, verified pack and optionally runs `hooks.save`. `--var` values become hook placeholders and `DRAFT_VAR_*` environment variables; built-ins cannot be overridden.

### `draft rollback <chk-id|pck-id|rcp-id>`

Rolls back by inferring target type from the ID prefix. Rollback protects `.draft/`.

## Receipts And Storage

### `draft receipt list`
### `draft receipt show <rcp-id>`

Inspects durable operation receipts. Receipt IDs use `rcp_`.

### `draft storage stats|gc|compact|prune|doctor`

Reports and maintains `.draft/` storage. Indexes, caches, and temporary data are rebuildable.
