# Command Reference

The Draft CLI is the primary interface. Core workflows are local-first and work without a daemon.

Human-readable output is the default CLI contract. Machine-readable output is available only where command help declares `--json` or `--raw`; v0.3.4 does not add those flags to every command.

The stable v0.3.4 contract centers on `init`, `doctor`, `config`, `event`, `receipt verify`, pack import/export/algebra, `verify`, `submit`, `rollback`, `close`, `gc`, Console, and extension management. `status`, `checkpoint`, `create`, `review`, `approve`, and `risk` are first-class local workflows using the same canonical ledger and contracts.

## Workspace

### `draft init [-b <base-pack-name>]`

Initializes `.draft/`, creates the project event stores, writes default config files, creates the base Pack, and selects it. The default base Pack name is `base`.

First-run output includes next actions: `draft task wizard`, `draft task list`, and `draft console`. If no command candidates are configured, Draft says so and points to `[candidates.<name>]` in `.draft/config.toml`; human/manual tasks still work without candidate setup.

### `draft status [-p <pck-id>] [-c repo|tasks|candidates|changes|hooks] [--full]`

Shows workspace or Pack status. Component filters return focused status for repository metadata, task health, resolved candidates, workspace changes, or hooks. `--full` includes backing records; the default stays compact. `.draft/` is always hard-excluded.

### `draft checkpoint <message>`

Creates a checkpoint with a `chk_` ID and a receipt.

## Doctor And Recovery

### `draft doctor [--global] [--json]`

Validates the project and global Draft stores, including metadata, event and receipt integrity, signing-key state, indexes, and recoverable journal operations. `--global` limits the report to the user-scoped store.

### `draft doctor sync [--fix] [--json]`

Inspects the global project registry. `--fix` removes or repairs stale registry entries; without it, the command is read-only.

### `draft doctor stats|gc|compact|prune [--json]`

Reports storage use or performs project maintenance. `gc` preserves active and recoverable state while removing unreachable objects, `compact` rewrites compactable stores, and `prune` removes safe rebuildable data.

### `draft doctor index [--refresh] [--global] [--json]`

Reports whether project or global derived indexes are fresh, stale, missing, or failed. `--refresh` rebuilds the selected scope before reporting it.

## Config, Hooks, And Ignore Rules

### `draft config get <key> [--global]`

### `draft config set <key> <value> [--global]`

### `draft config unset <key> [--global]`

Reads and writes config. Without `--global`, reads use project-over-global precedence and writes target the project. With `--global`, reads and writes target only the user-level `~/.draft/config.toml`.

The only mutable user profile is `user.name` and optional `user.email` through these commands. Both reject empty or whitespace-only values. Use `draft config unset user.email` for absence. Profile metadata cannot change the stable security actor, keys, signatures, authorization, trust, attribution, ownership, receipts, hashes, or digests.

### `draft hook set <key> <value>`

### `draft hook unset <key>`

### `draft hook run <hook-name>`

Manages project hook configuration. `hooks.submit` is the submit hook; inspect a configured value with `draft config get hooks.<key>`. Draft has no native commit, push, pull, sync, PR, MR, publish, host-specific, or remote commands.

### `draft ignore add|remove|list`

Manages `.draft/.ignore`. `.draft/` remains hard-excluded even if ignore rules are changed.

## Events

### `draft event [--page <page>] [--limit <entries>] [--raw]`

Renders a clean human-readable timeline derived from `.draft/events/event.log`. `--raw` prints the underlying compact JSONL event envelopes for audit, debugging, replay, and tooling. Use `draft doctor` or `draft receipt verify --all` to verify event, receipt, and transparency integrity. There is no `draft log`, and `draft event` accepts only long `--page` and `--limit` pagination flags.

## Packs

### `draft create <name> [-p <base-pck-id/name>]`

### `draft pack`

### `draft pack -s <pck-id/name>`

### `draft pack -d <pck-id/name>`

### `draft list`

Creates, shows, switches, deletes, and lists Packs. Pack IDs use `pck_`. Pack names must be unique among available Packs. `draft pack -d` asks for final `y/N` confirmation, removes the pack directory, removes task/run records owned only by that pack, garbage-collects unreachable objects, preserves events and receipts, selects a replacement pack when needed, and emits `pack.deleted`.

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

### `draft task update <task> [--status <state>] [--priority <priority>] [--due <RFC3339>|--clear-due] [--assignee <id> --assignee-kind actor|candidate|--clear-assignee]`

### `draft task next-action <task> add <label>` / `complete <action-id> [--reopen]`

### `draft task drop <task> [--hard]`

Creates, inspects, spawns, and retires task definitions. `draft task create` validates template ids, candidate presets, success criteria, zones, protected paths, and schema round-trips before writing. `draft task wizard` uses deterministic prompts for task name, template, goal, allowed/forbidden zones, success checks, risk, plan-first mode, and candidate preset; it prints a preview and only writes after confirmation. `task spawn` records task/candidate/Pack provenance and supports stored tasks, inline instructions, candidate presets, and execution lifecycle flags (`--resume`, `--cancel`, `--retry`). `task drop` clears execution/runtime state while keeping the definition; `--hard` removes the task definition and journals the operation.

### `draft inbox [--json]`

Lists items requiring attention: packs needing review, failed or resumable executions, owner review gaps, waiver renewals, doctor recovery warnings, and pending editor edits. Every item includes a next safe action.

### `draft waive <pck-id> <finding-id> --reason <text> --expires <duration>`

Creates an audited expiring waiver for a finding. Valid waivers participate in submit readiness and appear in inbox when renewal is near.

## Verify, Risk, Review, And Decisions

### `draft verify [-p <pck-id>]`

Verifies a Pack, defaulting to the selected Pack.

### `draft risk [-p <pck-id>] [--explain] [--include-evidence]`

Runs deterministic local risk analysis, defaulting to the selected Pack.

### `draft review [-p <pck-id>] [--tui]`

Starts review and locks the Pack for final human decision. `--tui` opens Draft Console in the terminal.

### `draft approve [-p <pck-id>]`

### `draft reject [-p <pck-id>]`

Records a mandatory human final decision. Review is required before approve/reject.

## Compare, Compose, Disperse, Submit, Rollback

### `draft compare <pck-a> <pck-b> [--tui]`

### `draft compose <pck-a> <pck-b> --output <name> [--tui]`

### `draft disperse <pck-id> --output <pack-a-name> <pack-b-name> [--tui]`

Compares, combines, or splits Packs with receipt-backed provenance.

### `draft submit [-p <pck-id>] [--dry-run] [--var key=value ...]`

Submits an approved, verified Pack and optionally runs `hooks.submit`. Before writing submit state, Draft verifies current evidence, canonical approval state, workspace hash, the canonical risk report (an unresolved `critical` risk blocks under the default policy), event chain, receipt signatures, transparency linkage, and protected-file exclusion. `--var` values become hook placeholders and `DRAFT_VAR_*` environment variables; built-ins cannot be overridden.

Submitting an **imported** pack additionally requires local re-verification and approval, applies the pack's embedded content to the workspace (fail closed: nothing is written if any touched file differs from the change's recorded base version), checkpoints the workspace first, and moves the pack out of quarantine. Submit hooks do not run for import submits.

Successful submits advance `stable_head` only after project-state verification and dispose mutable pack staging as the final step. The immutable manifest, every revision, lifecycle history, evidence, events, and receipts remain in Draft storage and can still be exported. Submit behavior is configured by `[submit].pack_disposal` in `.draft/config.toml` — `merge_and_dispose` (default) merges into Draft's stable base and advances `stable_head`; `dispose_only` delegates permanence to configured hooks and does not advance `stable_head`. See [Protocol Contracts](../internals/protocol.md).

### `draft rollback <chk-id|pck-id|rcp-id> [--dry-run]`

Rolls back by inferring target type from the ID prefix. `rcp_` references resolve only canonical signed receipts. The receipt must verify in the opened workspace ledger, carry a rollback-eligible event type (`CheckpointCreated`, `PackCreated`, `PackVerified`, `PackApproved`, or `PackSubmitted`), and identify an exact rollback snapshot. Rollback always protects `.draft/`.

### `draft close [--force]`

Removes Draft metadata from the workspace without deleting project files. Refuses pending unsafe state by default. `--force` discards Draft metadata even with pending packs but still leaves user files untouched.

### `draft gc`

Runs safe local maintenance: validates `stable_head`, preserves active and recoverable packs, removes safe temp/cache metadata, rebuilds indexes, and records gc receipts/events.

## Evidence Verification, Import/Export, And Pack Algebra

### `draft verify <pck-id|name> [--explain] [--full] [--fuzz]`

LSIF-backed evidence verification: assesses deterministic risk (persisted to `risk.json`, including the ML-ready feature vector), selects tests and fuzz targets, persists `verify.json`/`lsif.json`, sets the manifest evidence hashes, and records a signed `PackVerified` receipt. Policy escalates `--full`/`--fuzz` automatically for configured intents (default: `security`, `migration`). For imported packs, verification runs from the pack's embedded content objects and transitions it to `import_verified`.

### `draft pack --export <pck-id|name> [--output <path>]`

Writes a deterministic, uncompressed `.draftpack` with the stable format identifier `draftpack`. It contains the immutable manifest and revision, lifecycle, lockfile, patch, revision-bound evidence, signed receipts, provenance, and content-addressed patch objects. The header carries its registered Draftpack `schema_version` (version `1` in v0.3.4) and authenticates the complete member set. Signing keys, local trust decisions, and raw `.draft/` databases are never included.

### `draft pack --import <path> [--name <unique>] [--dry-run]`

Imports an untrusted `.draftpack` into quarantine with a separate typed trust record. Unsafe or unsupported archives are rejected before any member is promoted (see [Security](../internals/security.md)). Duplicate names require `--name`; duplicate pack ids are remapped. Review lifecycle remains `draft → verified → reviewing → approved → submitted` (or `reviewing → rejected`); quarantine trust is evaluated separately.

### `draft pack inspect <pck-id>` / `depends <pck-id>` / `conflicts <a> <b>` / `compose <a> <b> --name <name>`

Canonical pack algebra: lifecycle/evidence inspection, shared-symbol dependency analysis (LSIF-shortlisted), textual/semantic/policy/verification/dependency conflict detection, and composition.

## Console And Extensions

### `draft console [--port <n>] [--project <workspace-id|path>] [--no-open] [--no-preselect]`

Starts or reuses `draftd`, then serves Draft Console on an explicit loopback socket (default `127.0.0.1:4317`). It works outside a project; from inside one it preselects that registered workspace unless `--no-preselect` is set. `--project` accepts only an explicit path or opaque workspace id. `--no-open` leaves the one-time bootstrap URL in the terminal. See [Console](../guides/console.md).

### `draft service start|stop|restart|status [--json]`

Controls the long-lived local `draftd`. Core CLI workflows remain daemonless-capable; Console requires the daemon and reports a reconnectable offline state if IPC is unavailable.

### `draft project list|register|init|relocate|unregister|adopt-copy`

Manages the canonical global project registry. Relocation verifies the same immutable workspace id at the destination. `adopt-copy` creates an independent identity and preserves the copied Draft store in an adoption backup; it never merges histories.

### `draft pack reopen <pack-id> [--json]`

Reopens a verified, reviewing, approved, or rejected pack as a new audited mutable revision. Current verification/review bindings are invalidated while historical evidence remains. Submitted packs cannot be reopened; create a successor pack instead.

### `draft extension source add|list|remove|trust|refresh`

Configures local-directory or HTTPS catalogs without implicitly trusting them. `source trust <id> --root <file> --fingerprint sha256:...` is the explicit out-of-band trust bootstrap; `--reset` is an audited recovery action. Refresh requires a complete, unexpired signed root → timestamp → snapshot → targets chain, enforces signature thresholds and durable version floors, and rejects identity changes, rollback, replay, mix-and-match metadata, and delegation escape.

### `draft extension search [query] [--json]`

Searches cached signed targets. Expired cache remains inspectable with an explicit freshness state but cannot authorize installation or update.

### `draft extension install <path>` / `draft extension install <id> --source <source-id> [--version <version>]`

Installs either an explicitly selected local directory or a digest-authorized catalog archive. Every package passes the same declarative-only validator; Draft rejects commands, scripts, native code, executable permissions, links, unsafe paths, unknown contributions, and non-static assets.

### `draft extension update <id> --source <source-id> [--version <version>]` / `draft extension update --all`

Updates only from a currently usable trusted chain. Downloads are bounded, atomically cached, integrity checked, and promoted only after validation; the prior installed package is retained and restored after a failed promotion. Installed provenance remains durable if a source expires, becomes unavailable, or is removed.

### `draft extension list|show|uninstall|enable|disable`

Manages installed package state. Enablement activates declarative contributions only. Authoritative root revocation suppresses contributions and blocks re-enablement without deleting installation or provenance history. Draft never executes extension entrypoints.

## Receipts And Storage

### `draft receipt list`

### `draft receipt show <rcp-id>`

Inspects durable operation receipts. Receipt IDs use `rcp_`.

### `draft storage stats|gc|compact|prune|doctor`

Reports and maintains `.draft/` storage. Indexes, caches, and temporary data are rebuildable.
