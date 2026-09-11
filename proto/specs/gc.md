# GC Protocol

Garbage collection removes unreachable storage artifacts and derived or
transient state. It does **not** delete canonical accepted or history facts
merely because they are old, and `draft doctor` never collects anything.

## Reachability, not age

Every artifact is classified against a root graph:

```
Retained(Root)                  named directly by a root
Retained(ActiveJournal(j))      reachable from an unfinished transaction
Retained(UndrainedOutbox(f))    reachable from an audit fact awaiting its append
Retained(ReachableFrom(a))      reachable transitively from any of the above
Unknown { detail }              a reference GC could not resolve — never collected
Collectible                     reachable from nothing
```

`Unknown` is its own answer. An unreadable journal or an unresolvable reference
is not evidence of unreachability, and treating it as such is how a recovery
dependency gets deleted.

## Roots

Project metadata and `ProjectControlState`; every retained `BaselineRecord` and
reachable `BaselineManifest`, and all three roots per Baseline; the Resources,
`ResourceState` facts, retained `ResourceStateSemanticsContract` objects,
Observations, ObservationRuns, CoverageEvidence, RelationRecords and
StateBearingDeclarations those roots reference; Changes, ChangeDefinitions,
ScopeResolutions, sealed ChangeRevisions and Operations; Evidence, Assessments,
Representations, Reviews, Decisions, GateEvaluations and GateWaivers;
`ProjectSecurityState` objects and every fact a `SecurityFactRef` names;
PromotionRecords and PromotionReceipts; the Publication registry and every
`Publication` reachable through a `PublicationRef`, every `PublicationAttempt`
reachable through a `PublicationAttemptRef`, and the Outcomes, Resolutions,
RetryAuthorizations and Receipts beneath them; the Activity Ledger; transparency
records; active Workspaces and Checkpoints; every active Promotion, Publication,
creation, resolution, retry-authorization and initialization journal; **every
undrained audit outbox entry and unresolved `MutationJournal`**; RecoveryPlans;
and retained DraftPack manifests where configured.

## Anything an unfinished transaction needs is never collectible

A staged, never-dispatched `PublicationAttempt` artifact is exactly the case
this exists for. It is not proof that anything was sent, it has no primary
outcome, and it still may not be collected while its journal or control recovery
is outstanding — because the journal is the record of what has to be finished,
and collecting what it names would lose the instructions.

It becomes collectible only once journal and control recovery are complete and
no other root needs it. The same rule releases the candidate journal of a
terminal `Abandoned` attempt: the disposition is durable and carries its own
classification evidence, so nothing further depends on the journal remaining.

## The sweep

`draft maintenance gc` acquires the maintenance lock, validates accepted-Baseline
integrity, classifies the root graph, deletes only what classified as
`Collectible`, rebuilds indexes, and records `MaintenanceStarted` followed by
`MaintenanceCompleted` or `MaintenanceFailed`.

`gc_objects_marked` and `gc_objects_collected` are counted separately on
purpose: a mark that never becomes a collection is the symptom of a sweep that
keeps failing. `gc_recovery_roots_preserved` counts retention owed to an
unfinished transaction rather than to ordinary history, which is what tells an
operator that recovery work is holding storage.

<!-- retired-architecture-ok: naming the removed command is the point. -->
`draft maintenance prune` does not exist in v1.
