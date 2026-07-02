# Concepts

Draft v0.3.1 is organized around local, reviewable ChangePacks.

## Workspace

A Draft workspace is a project directory with a `.draft/` store. Draft metadata is private project state and is always excluded from user change candidates.

## Checkpoint

A checkpoint records a baseline snapshot of workspace content. Later ChangePacks compare workspace changes against a snapshot.

## ChangePack

A ChangePack is Draft’s reviewable unit of change. It contains patch references, evidence, verification and risk results, review decisions, approval state, receipts, and provenance.

## Event

Draft stores raw event records as JSON Lines under `.draft/events/`. `draft event` renders those records as a human-readable timeline. `draft event --raw` prints the underlying JSONL records.

## Receipt

A receipt is a durable record of an operation such as checkpoint, save, rollback, storage maintenance, or hook execution.

## Candidate And Task

Candidates are local execution profiles. Tasks record intent and can link instructions, candidates, ChangePacks, and evidence.

## Hook

A hook is a user-configured shell command. Draft records hook execution but does not treat hook contents as native Git, host, deployment, or remote behavior.

