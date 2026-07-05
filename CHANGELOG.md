# Changelog

All notable public changes to Draft are tracked here.

## v0.3.2

Draft v0.3.2 turns Draft into a verified **changepack system**. Changepacks are
not branches — they are independent, composable, portable, signed, locally
verifiable units of change.

### Added

- Two hidden metadata stores: global `~/.draft/` and project `<root>/.draft/`,
  with config/policy precedence (CLI > project > global > default).
- Ed25519-signed receipts, a hash-chained canonical event log, and a
  tamper-evident transparency chain; `draft receipt verify <rcp_id>` / `--all`.
- Canonical pack `manifest.json` + `pack.lock.json`, ten intents, and a
  local/imported state model.
- Portable `.draftpack` import/export (format `draftpack/2`) with quarantine,
  unique-name enforcement (`--name`), `--dry-run`, and a hardened
  untrusted-import boundary (path traversal, absolute, `.draft/`, symlink,
  hardlink, device, invalid UTF-8, oversized, zip-bomb, wrong-schema receipts,
  tampered content objects — all rejected fail-closed).
- Content-embedded exports: a `.draftpack` carries the content-addressed
  objects its patch references, so an importing workspace re-verifies the
  actual change content and `draft save` applies it — conflict-checked
  against each file's recorded base version, checkpointed first, and promoted
  out of quarantine on success.
- The full imported-pack lifecycle: `imported_quarantined → import_verified →
  import_approved → import_saved` (or terminal `import_rejected`), driven by
  `draft verify`, `draft approve`/`reject`, and `draft save`; origin trust
  marks are stripped at import.
- Enforced policy layer with field-level precedence (project > global > safe
  default): approval-for-save, critical-risk blocking, high-risk approval,
  workspace re-verification, local re-verification of imports, and
  intent-based full/fuzz verification escalation; malformed policy files fail
  closed.
- Rollback by canonical signed receipt: `draft rollback rcp_<id>` verifies the
  receipt and resolves rollback-eligible event types through their subject
  (legacy rollback receipts keep working).
- The persisted risk report now includes the ML-ready feature vector, real
  dependency counts, and candidate rollback-rate signals.
- `draft doctor --global` lists every protocol adapter's status; `acp-comm`
  (Agent Communication Protocol) is explicitly experimental.
- Explainable rule-first risk model, a basic offline LSIF symbol index, and
  evidence-based test/fuzz selection: `draft verify <pck_id> --explain/--full/--fuzz`.
- Pack algebra: `draft pack inspect|depends|conflicts|compose`.
- `draft init --global`, `draft doctor [--global]`, `draft identity status`,
  `draft config get/set [--global]`, `draft save --dry-run`,
  `draft rollback --dry-run`.
- AG-UI Review Cockpit (`draft cockpit`) — a local-only browser UI with CSRF
  protection and no key exposure.
- Real MCP/ACP/A2A adapters (`draft mcp`, `draft acp`, `draft a2a`).
- Criterion benchmark suite, stable security-fixture tests, and nightly
  cargo-fuzz targets for the parsers.

### Safety

- Every `.draft/` path (any nesting, case-insensitive) is hard-excluded from
  pack/diff/import/export/save/rollback/scan operations.
- The private signing key lives only in `~/.draft/keys/signing.key` (0600).
- Fail closed on any trust/path/hash/receipt/event verification failure.

### Compatibility

- No `draft log`; `draft event` remains the event surface with only `--page`
  and `--limit` (no `-p`/`-l`). Rollback still accepts `chk_`/`pck_`/`rcp_`.
- One-time, idempotent migration of an existing v0.3.1 `.draft/`.

## v0.3.1

Draft v0.3.1 focuses on local verified ChangePacks, review, approval, receipts, rollback, and public documentation readiness.

### Added

- Native `.draft/` workspace store for config, objects, snapshots, ChangePacks, events, receipts, evidence, tasks, runs, and rebuildable indexes.
- Local ChangePack flow: checkpoint, create, verify, risk, review, approve or reject, save, receipt inspection, and rollback.
- Append-only hash-chained event stream with human-readable and raw `draft event` output.
- `draft event --raw` for JSONL event records and a normal human-readable timeline derived from those records.
- Candidate and task commands for local execution profiles and task provenance.
- Optional opaque `hooks.save` execution after Draft approval, policy, and `.draft/` safety checks.
- Storage maintenance commands and local service crates for optional background flows.

### Safety

- `.draft/` is hard-excluded from status, snapshots, ChangePacks, save candidates, rollback plans, watcher paths, and hook candidate checks.
- Failed saves record receipts and events.
- Hooks are captured as command evidence but are not interpreted as native Git, host, deployment, or remote operations.

### Documentation

- Public README, user guides, command reference, safety model, release compliance, support, conduct, brand, and roadmap documentation.
