# Concepts

Draft v0.3.3 is organized around local, reviewable, signed changepacks.

## Workspace

A Draft workspace is a project directory with a `.draft/` store. Draft metadata is private project state and is always excluded from user change candidates.

## Checkpoint

A checkpoint records a baseline snapshot of workspace content. Later ChangePacks compare workspace changes against a snapshot.

## ChangePack

A ChangePack is Draft’s reviewable unit of change. It contains patch references, evidence, verification and risk results, review decisions, approval state, signed receipts, and provenance.

## Event

Draft stores raw event records as JSON Lines under `.draft/events/`. `draft event` renders those records as a human-readable timeline. `draft event --raw` prints the underlying JSONL records.

## Receipt

A receipt is a signed durable record of a trust-relevant operation such as checkpoint, verify, approve, save, import/export, rollback, storage maintenance, or hook execution.

## Imported Pack

A `.draftpack` artifact carries a pack's manifest, patch, evidence, signed receipts, and the content-addressed objects its patch references. Imports are untrusted: they enter `imports/quarantine/`, lose all origin trust marks, and follow their own lifecycle — `imported_quarantined → import_verified → import_approved → import_saved` (or terminal `import_rejected`). Saving an approved import applies its content to the workspace only when every touched file matches the change's recorded base version.

## Candidate And Task

Candidates are local execution profiles. Tasks record intent and can link instructions, candidates, ChangePacks, and evidence.

## Hook

A hook is a user-configured shell command. Draft records hook execution but does not treat hook contents as native Git, host, deployment, or remote behavior.
