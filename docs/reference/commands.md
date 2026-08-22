# Command Reference

The Draft CLI is the primary v0.3.4 interface. Core workflows are local-first and work without a daemon.

Human-readable output is the default CLI contract. Machine-readable JSON is available only through existing supported `--raw` command surfaces; v0.3.4 does not add `--raw` or JSON output to every command.

The stable v0.3.4 contract centers on `init`, `doctor`, `identity`, `config`, `event`, `receipt verify`, `pack import/export/algebra`, `verify`, `submit`, `rollback`, `close`, `gc`, `console`, and extension management. Earlier local workflow commands such as `status`, `checkpoint`, `create`, `review`, `approve`, and `risk` remain available as compatibility commands and route trust-relevant state through the canonical v0.3.4 ledger.

## Workspace

### `draft init [-b <base-pack-name>]`

Initializes `.draft/`, creates the project event stores, writes default config files, creates the base ChangePack, and selects it. The default base ChangePack name is `base`.

First-run output includes next actions: `draft task wizard`, `draft task list`, and `draft console`. If no command candidates are configured, Draft says so and points to `[candidates.<name>]` in `.draft/config.toml`; human/manual tasks still work without candidate setup.

### `draft status [-p <pck-id>] [-c repo|tasks|candidates|changes|hooks] [--full]`

Shows workspace or ChangePack status. Component filters return focused status for repository metadata, task health, resolved candidates, workspace changes, or hooks. `--full` includes backing records; the default stays compact. `.draft/` is always hard-excluded.

### `draft checkpoint <message>`

Creates a checkpoint with a `chk_` ID and a receipt.

## Doctor And Identity

### `draft doctor [--global] [--json]`

Validates the project and global Draft stores, including metadata, event and receipt integrity, signing-key state, indexes, and recoverable journal operations. `--global` limits the report to the user-scoped store.

### `draft doctor sync [--fix] [--json]`

Inspects the global project registry. `--fix` removes or repairs stale registry entries; without it, the command is read-only.

### `draft doctor stats|gc|compact|prune [--json]`

Reports storage use or performs project maintenance. `gc` preserves active and recoverable state while removing unreachable objects, `compact` rewrites compactable stores, and `prune` removes safe rebuildable data.

### `draft doctor index [--refresh] [--global] [--json]`

Reports whether project or global derived indexes are fresh, stale, missing, or failed. `--refresh` rebuilds the selected scope before reporting it.

### `draft doctor migrate [--check] [--json]`

Reports and, unless `--check` is supplied, applies the project migration to the current Draft version. Migration first records byte-for-byte backups under `.draft/backups/migration-*` for `workspace.json`, `config.toml` when present, and every stored or quarantined pack manifest. It validates all submit-name and schema transformations before writing any source file, commits with atomic replacements, and restores the originals if a commit fails. Pending and quarantined packs are preserved, and rerunning after success is a no-op.

### `draft identity status [--json]`

Shows the active actor identity, its source, and whether the signing key is available. Identity inspection is user-scoped and does not require a project workspace.

## Config, Hooks, And Ignore Rules

### `draft config get <key> [--global]`

### `draft config set <key> <value> [--global]`

### `draft config unset <key> [--global]`

Reads and writes config. Without `--global`, reads use project-over-global precedence and writes target the project. With `--global`, reads and writes target only the user-level `~/.draft/config.toml`.

### `draft hook get <key> [--global]`

### `draft hook set <key> <value> [--global]`

### `draft hook unset <key> [--global]`

### `draft hook run <hook-name>`

Manages hook configuration. `hooks.submit` is the submit hook; Draft has no native commit, push, pull, sync, PR, MR, publish, host-specific, or remote commands.

### `draft ignore add|remove|list`

Manages `.draft/.ignore`. `.draft/` remains hard-excluded even if ignore rules are changed.

## Events

### `draft event [--page <page>] [--limit <entries>] [--raw]`

Renders a clean human-readable timeline derived from `.draft/events/event.log`. `--raw` prints the underlying compact JSONL event envelopes for audit, debugging, replay, and tooling. Use `draft doctor` or `draft receipt verify --all` to verify event, receipt, and transparency integrity. There is no `draft log`, and `draft event` accepts only long `--page` and `--limit` pagination flags.

## ChangePacks

### `draft create <name> [-p <base-pck-id/name>]`

### `draft pack`

### `draft pack -s <pck-id/name>`

### `draft pack -d <pck-id/name>`

### `draft list`

Creates, shows, switches, deletes, and lists ChangePacks. ChangePack IDs use `pck_`. ChangePack names must be unique among available ChangePacks. `draft pack -d` asks for final `y/N` confirmation, removes the pack directory, removes task/run records owned only by that pack, garbage-collects unreachable objects, preserves events and receipts, selects a replacement pack when needed, and emits `pack.deleted`.

## Candidates And Tasks

### `draft candidate list|show|remove`

### `draft candidate add <name> [--kind command|chat|manual] -- <template>`

### `draft candidate update <name> [--kind command|chat|manual] -- <template>`

### `draft candidate packs [-p <pck-id>] [-c <candidate-name>]`

Manages host-agnostic candidate execution profiles. Missing candidates referenced by task spawn are auto-registered.

### `draft task spawn "<name>" [-p <pck-id>] [-c <candidate-name> ...] [--cron <expr>] -- <instruction>`

### `draft task create <name> --goal <goal> [--template <id>] [--candidate-preset <id>]`

### `draft task wizard`

### `draft task list`

### `draft task show <task> [--full]`

### `draft task drop <task> [--hard]`

### `draft task <task>`

Creates, inspects, spawns, and retires task definitions. `draft task create` validates template ids, candidate presets, success criteria, zones, protected paths, and schema round-trips before writing. `draft task wizard` uses deterministic prompts for task name, template, goal, allowed/forbidden zones, success checks, risk, plan-first mode, and candidate preset; it prints a preview and only writes after confirmation. `task spawn` records task/candidate/ChangePack provenance and supports stored tasks, inline instructions, candidate presets, and execution lifecycle flags (`--resume`, `--cancel`, `--retry`). `task drop` clears execution/runtime state while keeping the definition; `--hard` removes the task definition and journals the operation.

### `draft inbox [--json]`

Lists items requiring attention: packs needing review, failed or resumable executions, owner review gaps, waiver renewals, doctor recovery warnings, and pending editor edits. Every item includes a next safe action.

### `draft waive <pck-id> <finding-id> --reason <text> --expires <duration>`

Creates an audited expiring waiver for a finding. Valid waivers participate in submit readiness and appear in inbox when renewal is near.

## Verify, Risk, Review, And Decisions

### `draft verify [-p <pck-id>]`

Verifies a ChangePack, defaulting to the selected ChangePack.

### `draft risk [-p <pck-id>] [--explain] [--include-evidence]`

Runs deterministic local risk analysis, defaulting to the selected ChangePack.

### `draft review [-p <pck-id>] [--tui]`

Starts review and locks the ChangePack for final human decision. `--tui` opens Draft Console in the terminal.

### `draft approve [-p <pck-id>]`

### `draft reject [-p <pck-id>]`

Records a mandatory human final decision. Review is required before approve/reject.

## Compare, Compose, Disperse, Submit, Rollback

### `draft compare <pck-a> <pck-b> [--tui]`

### `draft compose <pck-a> <pck-b> --output <name> [--tui]`

### `draft disperse <pck-id> --output <pack-a-name> <pack-b-name> [--tui]`

Compares, combines, or splits ChangePacks with receipt-backed provenance.

### `draft submit [-p <pck-id>] [--dry-run] [--var key=value ...]`

Submits an approved, verified ChangePack and optionally runs `hooks.submit`. Before writing submit state, Draft verifies current evidence, canonical approval state, workspace hash, the canonical risk report (an unresolved `critical` risk blocks under the default policy), event chain, receipt signatures, transparency linkage, and protected-file exclusion. `--var` values become hook placeholders and `DRAFT_VAR_*` environment variables; built-ins cannot be overridden.

Submitting an **imported** pack additionally requires local re-verification and approval, applies the pack's embedded content to the workspace (fail closed: nothing is written if any touched file differs from the change's recorded base version), checkpoints the workspace first, and moves the pack out of quarantine. Submit hooks do not run for import submits.

Successful submits advance `stable_head` only after project-state verification and dispose changepack metadata as the final step. Export portable packs before submit if you need to retain the full payload outside active Draft storage. Submit behavior is configured by `[submit].pack_disposal` in `.draft/config.toml` — `merge_and_dispose` (default) merges into Draft's stable base and advances `stable_head`; `dispose_only` delegates permanence to configured hooks and does not advance `stable_head`. See [Protocol Contracts](../internals/protocol.md).

### `draft rollback <chk-id|pck-id|rcp-id> [--dry-run]`

Rolls back by inferring target type from the ID prefix. `rcp_` references resolve both legacy rollback receipts and canonical signed receipts; a canonical receipt must verify and carry a rollback-eligible event type (`CheckpointCreated`, `PackCreated`, `PackVerified`, `PackApproved`, `PackSubmitted`), and resolves through its subject. Rollback protects `.draft/`.

### `draft close [--force]`

Removes Draft metadata from the workspace without deleting project files. Refuses pending unsafe state by default. `--force` discards Draft metadata even with pending packs but still leaves user files untouched.

### `draft gc`

Runs safe local maintenance: validates `stable_head`, preserves active and recoverable packs, removes safe temp/cache metadata, rebuilds indexes, and records gc receipts/events.

## Evidence Verification, Import/Export, And Pack Algebra

### `draft verify <pck-id|name> [--explain] [--full] [--fuzz]`

LSIF-backed evidence verification: assesses deterministic risk (persisted to `risk.json`, including the ML-ready feature vector), selects tests and fuzz targets, persists `verify.json`/`lsif.json`, sets the manifest evidence hashes, and records a signed `PackVerified` receipt. Policy escalates `--full`/`--fuzz` automatically for configured intents (default: `security`, `migration`). For imported packs, verification runs from the pack's embedded content objects and transitions it to `import_verified`.

### `draft pack --export <pck-id|name> [--output <path>]`

Writes a deterministic, uncompressed `.draftpack` (format `draftpack/2`) containing the manifest, lockfile, patch, evidence, signed receipts, provenance, and the content-addressed objects referenced by the patch. Never includes signing keys, trust data, or raw `.draft/` databases. Emits a signed `PackExported` receipt.

### `draft pack --import <path> [--name <unique>] [--dry-run]`

Imports an untrusted `.draftpack` into `imports/quarantine/` as `imported_quarantined`, stripping all origin trust marks. Unsafe archives are rejected fail-closed (see [Security](../internals/security.md)). Duplicate names require `--name`; duplicate pack ids are remapped. The imported lifecycle is `imported_quarantined → import_verified → import_approved → import_submitted` (or `import_rejected`, which is terminal), driven by `draft verify`, `draft approve`/`draft reject`, and `draft submit`.

### `draft pack inspect <pck-id>` / `depends <pck-id>` / `conflicts <a> <b>` / `compose <a> <b> --name <name>`

Canonical pack algebra: lifecycle/evidence inspection, shared-symbol dependency analysis (LSIF-shortlisted), textual/semantic/policy/verification/dependency conflict detection, and composition.

## Console And Extensions

### `draft console [--port <n>]`

Serves Draft Console on loopback (default `127.0.0.1:4317`) with pack list/detail, risk, diff, events, receipts, approve/reject, and import/export. Mutations require the per-session CSRF token.

### `draft extension list|show|install|uninstall|enable|disable`

Manages versioned local extension packages and their enabled state. Draft does not execute extension entrypoints.

## Receipts And Storage

### `draft receipt list`

### `draft receipt show <rcp-id>`

Inspects durable operation receipts. Receipt IDs use `rcp_`.

### `draft storage stats|gc|compact|prune|doctor`

Reports and maintains `.draft/` storage. Indexes, caches, and temporary data are rebuildable.
