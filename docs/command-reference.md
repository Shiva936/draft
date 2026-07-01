# Command Reference

The Draft CLI is the primary v0.3.0 interface. Every core command works without `draftd`; service-backed flows are optional.

Most read commands support `--json`. Human output is intended for terminals; JSON output is intended for scripts and TUI/service integration.

## Workspace

### `draft init`

Initializes `.draft/`, default config, policy, verification files, event log, object store, index, and workspace metadata.

### `draft status`

Scans the workspace directly and compares it to the latest snapshot. The scan includes files unknown to external systems and always excludes `.draft/`.

### `draft checkpoint <message>`

Creates a snapshot and receipt. Use checkpoints before agent runs, large edits, or rollback-sensitive work.

### `draft index rebuild`

Rebuilds the local SQLite index from durable store files. Indexes are cache data; the JSON/JSONL store remains actoritative.

## Configuration And Ignore Rules

### `draft config set|get|unset|list`

Reads and writes Draft config. Supported v0.3.0 keys include:

- `identity.username`
- `identity.email`
- `hooks.save`
- `hooks.verify`
- `save.message_template`

Draft v0.3.0 has no external-action config keys. User-owned remote commands may be configured only as opaque hook command text.

Hook-capable commands accept `--var` as a tail marker for dynamic hook variables:

```bash
draft save auth-refactor --var ticket="AUTH-123" release="v0.3.0"
```

Every token after `--var` must be `key=value`; normal Draft flags are not allowed after it.

### `draft ignore add|remove|list`

Manages `.draft/.ignore`, Draft’s own ignore file. `.draft/` is hard-excluded even if the ignore file is edited.

## Tasks, Runs, And Evidence

### `draft task create|list|show`

Creates and inspects local task records. Tasks can be linked to changepacks.

### `draft spawn`

Runs an opaque command from the workspace root and captures evidence. This is useful for agent or script execution where output should be attached to the Draft record.

### `draft runs`

Lists and inspects captured runs.

## Changepacks

### `draft pack create --from-working-tree`

Creates a changepack from the current workspace delta.

### `draft pack list`

Lists changepacks with current status.

### `draft pack show <pack-id>`

Shows the changepack manifest and references.

## Verification, Risk, And Review

### `draft verify <pack-id>`

Runs configured checks and stores result evidence. The default policy requires verification before save.

### `draft risk <pack-id>`

Computes local risk findings from file changes, sensitive paths, binary changes, deletions, and related signals.

### `draft review <pack-id>`

Shows review state. With `--comment`, stores a review comment.

### `draft approve <pack-id>`

Records an approval decision.

### `draft reject <pack-id>`

Records a rejection decision.

## Compare, Compose, Save, Rollback

### `draft compare <left> <right>`

Compares changepacks and reports overlaps.

### `draft compose <left> <right> --output <name>`

Creates a new changepack from compatible sources. Overlapping changes are rejected.

### `draft save <pack-id>`

Saves an approved, verified changepack into `.draft/`, optionally runs `hooks.save`, and records a save receipt. `hooks.save` may be a raw command string or rich hook entry. Draft aborts before hook execution if `.draft/` is present in the save candidate. Hook placeholders use `{{name}}`.

### `draft rollback <snapshot-or-receipt> --plan`

Previews affected files.

### `draft rollback <snapshot-or-receipt> --yes`

Applies rollback after explicit confirmation.

## Receipts And Events

### `draft receipt list|show`

Inspects durable receipts for checkpoint, verification, save, compose, and rollback operations.

### `draft events`

Shows append-only hash-chained events.

### `draft events --verify-chain`

Verifies event hash links and reports tampering or parse failures.

## Services

### `draft service status|start|stop`

Controls the optional local daemon. The daemon is a convenience layer, not a requirement for core CLI operation.
