# Operation log

A Draft-owned, append-only, integrity-checked history under `.draft/operations/` — independent of any provider-native log (ADR 0003).

## Records
Each `NNNNNNNNNNNNNNNN.operation.json` contains: id + monotonic seq, workspace id, parent ids, actor, provider id, observed provider view, timestamp, kind, input/output refs, optional risk/verification summaries, receipt refs, and a sha256 `integrity` hash (over the canonical body, excluding the hash itself).

## Kinds
WorkspaceDetected/Initialized, ProviderSelected, ChangeScanned/Grouped, ReviewStarted/ReviewDecisionRecorded, RiskEvaluated,
VerificationStarted/Completed, CheckpointCreated/Restored, FinalizationPlanned/Completed, ProviderPublished, UndoPlanned/Applied, ServiceStarted/Stopped, WorkspaceMigrated.

## Append protocol
Acquire the append lock → allocate next seq → write a temp file → fsync → atomic rename → update `index.json` → release. The index is rebuildable from the records. Corrupt records are detected on read.

## Who appends
`core::App` writes the records the engines don't self-append; the checkpoint and finalization engines self-append their own. `draftd` logs service lifecycle.
