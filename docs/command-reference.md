# Command Reference

The Draft CLI is the primary v0.3.2 interface. Core workflows are local-first and work without a daemon.

Most read commands support `--json`. Human output is intended for terminals; JSON output is intended for scripts and TUI integration.

The stable v0.3.2 contract centers on `init`, `doctor`, `identity`, `config`,
`event`, `receipt verify`, `pack` import/export/algebra, `verify`, `save`,
`rollback`, `cockpit`, and adapter commands. Earlier local workflow commands
such as `status`, `checkpoint`, `create`, `review`, `approve`, and `risk` remain
available as compatibility commands and route trust-relevant state through the
canonical v0.3.2 ledger.

## Workspace

### `draft init [-b <base-pack-name>]`

Initializes `.draft/`, creates the project event stores, writes default config files, creates the base ChangePack, and selects it. The default base ChangePack name is `base`.

### `draft status [-p <pck-id>] [-c repo|tasks|candidates|changes|hooks] [--full]`

Shows workspace or ChangePack status. `.draft/` is always hard-excluded.

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

## Events

### `draft event [--page <page>] [--limit <entries>] [--raw] [--json]`

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
### `draft task list`
### `draft task`

Spawns tasks and records task/candidate/ChangePack provenance. `draft task` shows the latest task or reports that no task is running.

## Verify, Risk, Review, And Decisions

### `draft verify [-p <pck-id>]`

Verifies a ChangePack, defaulting to the selected ChangePack.

### `draft risk [-p <pck-id>] [--explain] [--include-evidence]`

Runs deterministic local risk analysis, defaulting to the selected ChangePack.

### `draft review [-p <pck-id>] [--tui]`

Starts review and locks the ChangePack for final human decision. `--tui` opens the Review Cockpit.

### `draft approve [-p <pck-id>]`
### `draft reject [-p <pck-id>]`

Records a mandatory human final decision. Review is required before approve/reject.

## Compare, Compose, Disperse, Save, Rollback

### `draft compare <pck-a> <pck-b> [--tui]`
### `draft compose <pck-a> <pck-b> --output <name> [--tui]`
### `draft disperse <pck-id> --output <pack-a-name> <pack-b-name> [--tui]`

Compares, combines, or splits ChangePacks with receipt-backed provenance.

### `draft save [-p <pck-id>] [--dry-run] [--var key=value ...]`

Saves an approved, verified ChangePack and optionally runs `hooks.save`. Before writing save state, Draft verifies current evidence, canonical approval state, workspace hash, the canonical risk report (an unresolved `critical` risk blocks under the default policy), event chain, receipt signatures, transparency linkage, and `.draft/` exclusion. `--var` values become hook placeholders and `DRAFT_VAR_*` environment variables; built-ins cannot be overridden.

Saving an **imported** pack additionally requires local re-verification and approval, applies the pack's embedded content to the workspace (fail closed: nothing is written if any touched file differs from the change's recorded base version), checkpoints the workspace first, and moves the pack out of quarantine. Save hooks do not run for import saves.

### `draft rollback <chk-id|pck-id|rcp-id> [--dry-run]`

Rolls back by inferring target type from the ID prefix. `rcp_` references resolve both legacy rollback receipts and canonical signed receipts; a canonical receipt must verify and carry a rollback-eligible event type (`CheckpointCreated`, `PackCreated`, `PackVerified`, `PackApproved`, `PackSaved`), and resolves through its subject. Rollback protects `.draft/`.

## Evidence Verification, Import/Export, And Pack Algebra

### `draft verify <pck-id|name> [--explain] [--full] [--fuzz]`

LSIF-backed evidence verification: assesses deterministic risk (persisted to `risk.json`, including the ML-ready feature vector), selects tests and fuzz targets, persists `verify.json`/`lsif.json`, sets the manifest evidence hashes, and records a signed `PackVerified` receipt. Policy escalates `--full`/`--fuzz` automatically for configured intents (default: `security`, `migration`). For imported packs, verification runs from the pack's embedded content objects and transitions it to `import_verified`.

### `draft pack --export <pck-id|name> [--output <path>]`

Writes a deterministic, uncompressed `.draftpack` (format `draftpack/2`) containing the manifest, lockfile, patch, evidence, signed receipts, provenance, and the content-addressed objects referenced by the patch. Never includes signing keys, trust data, or raw `.draft/` databases. Emits a signed `PackExported` receipt.

### `draft pack --import <path> [--name <unique>] [--dry-run]`

Imports an untrusted `.draftpack` into `imports/quarantine/` as `imported_quarantined`, stripping all origin trust marks. Unsafe archives are rejected fail-closed (see [security.md](security.md)). Duplicate names require `--name`; duplicate pack ids are remapped. The imported lifecycle is `imported_quarantined → import_verified → import_approved → import_saved` (or `import_rejected`, which is terminal), driven by `draft verify`, `draft approve`/`draft reject`, and `draft save`.

### `draft pack inspect <pck-id>` / `depends <pck-id>` / `conflicts <a> <b>` / `compose <a> <b> --name <name>`

Canonical pack algebra: lifecycle/evidence inspection, shared-symbol dependency analysis (LSIF-shortlisted), textual/semantic/policy/verification/dependency conflict detection, and composition.

## Cockpit And Adapters

### `draft cockpit [--port <n>]`

Serves the loopback-only AG-UI Review Cockpit (default `127.0.0.1:4317`) with pack list/detail, risk, diff, events, receipts, approve/reject, and import/export. Mutations require the per-session CSRF token.

### `draft mcp` / `draft acp <op>` / `draft a2a <op>`

Protocol adapters. MCP exposes safe read/evidence tools over stdio and refuses save/rollback/approve (human approval only). ACP handles request-approval/approve/reject/list-pending through core with signed receipts. A2A registers candidates and records provenance links. Adapter statuses (including the experimental `acp-comm` Agent Communication Protocol) are listed by `draft doctor --global`.

## Receipts And Storage

### `draft receipt list`
### `draft receipt show <rcp-id>`

Inspects durable operation receipts. Receipt IDs use `rcp_`.

### `draft storage stats|gc|compact|prune|doctor`

Reports and maintains `.draft/` storage. Indexes, caches, and temporary data are rebuildable.
