# Storage And Events

Draft stores private state in two hidden `.draft/` stores. The global store (`~/.draft/`) holds the stable security actor, signing keys, trust data, global configuration, adapter config, and reusable cache state. The project store (`<workspace>/.draft/`) holds ChangePacks, Activity, receipts, transparency data, checkpoints, evidence, and indexes. Both stores are native to Draft v0.3.4 and independent of external tools.

## Global Store

- `~/.draft/config.toml`: global defaults, including optional `user.*` display/contact metadata.
- `~/.draft/identity/`: stable security actor and candidate registry; profile fields are forbidden here.
- `~/.draft/keys/signing.key`: private Ed25519 signing key.
- `~/.draft/trust/`: trusted and revoked public-key metadata.
- `~/.draft/adapters/`: local adapter configuration.
- `~/.draft/cache/`: rebuildable global caches.

The global store never stores project ChangePack data.

## Project Top-Level Files

- `.draft/project.json`: project id, its contract schema version, product/provenance `draft_version`, and creation time. `draft_version` does not determine workspace compatibility.
- `.draft/config.toml`: workspace configuration.
- `.draft/.ignore`: Draft ignore rules.
- `.draft/verify.toml`: verification command configuration.
- `.draft/policy.toml`: promotion, review, and evidence policy.

## Durable Directories

- `.draft/events/`: the append-only, hash-chained Activity Ledger (`events.log` is authoritative; `events.index` is derived).
- `.draft/objects/`: content-addressed blobs for file contents, stdout, stderr, messages, and evidence.
- `.draft/snapshots/`: workspace manifests created by checkpoints and rollback-sensitive operations.
- `.draft/tasks/`: local task records.
- `.draft/executions/`: canonical task execution records.
- `.draft/change-pack-workspaces/`: mutable staging state while deriving immutable revisions.
- `.draft/evidence/`: verification and run evidence.
- `.draft/receipts/`: signed receipts; `receipts/envelopes/` holds each `rcp_` envelope create-once, bound to the digest of its own canonical bytes.
- `.draft/transparency/`: local tamper-evident receipt/event chain.
- `.draft/graph/change-packs/`: the revisioned `ChangePack` records, each with its stable lock sidecar.
- `.draft/definitions/`, `.draft/resolutions/`, `.draft/packs/revision/`: immutable `ChangePackDefinition`, `ScopeResolution` and sealed `RevisionPack` facts, each under the create-once `LogicalId → CanonicalPayloadDigest` binding.
- `.draft/representations/`: the derived explanation of each sealed revision, keyed by revision — one revision has one explanation.
- `.draft/assessments/`, `.draft/gates/`, `.draft/reviews/`, `.draft/decisions/`, `.draft/waivers/`: the judgement chain, every fact bound to one exact revision.
- `.draft/baselines/`, `.draft/observations/`: accepted Baselines with their three roots, and the canonical observations and runs that establish them.
- `.draft/semantics-contracts/`: retained `ResourceStateSemanticsContract` objects. A GC root in their own right — verifying what a historical state _meant_ must not require an installed extension.
- `.draft/provider-bindings/`, `.draft/provider-definitions/`: the revisioned pointer, and the immutable definitions and profiles it points at. The definitions are retained forever, because a Baseline's provenance names one.
- `.draft/security/{grants,revocations,states}/`, `.draft/promotions/{journal,records}/`: authority facts, and the journalled path that advances an accepted Baseline.
- `.draft/publication/`: the whole Publication subtree — `registry/`, `publications/`, `attempts/`, `journal/`, `control/`, `outcome-heads/`, `resolutions/` and `retry-authorizations/`, each with its own stable lock sidecar. The six sidecars have distinct, non-interchangeable purposes and are never conflated.
- `.draft/packs/`: canonical Pack objects, and nothing else — `packs/change/<cpk_>/` holds a ChangePack's manifest, content revisions, lockfile and per-Pack files; `packs/revision/` holds the sealed, create-once RevisionPack facts. The authoritative ChangePack _record_ (identity, lifecycle, current definition) is `graph/change-packs/<cpk_>.json`, CAS-guarded.
- `.draft/exports/`: what the filesystem Publication provider delivers — an external effect written outside the graph.
- `.draft/impact/`: the offline index of contributed elements and their relations.
- `.draft/recovery-anchors/`: retained material for restoring a past state, keyed by that state's digest.
- `.draft/observation-provenance/<state digest>/<provenance digest>.json`: which implementation actually performed each observation. A directory rather than a file, because one observed state may have many immutable records — the same state observed again later, or by a semantics-equivalent build, is a different historical observation, and a receipt that relied on the first must keep pointing at the first. Naming each record by its own digest makes the store append-only and re-recording an identical assembly idempotent. None of it is an input to any state digest.
- `.draft/indexes/`: rebuildable SQLite indexes.
- `.draft/locks/`: local writer locks.
- `.draft/tmp/`: temporary files for atomic writes.

The project store never stores the private signing key.

## Contract And Container Boundaries

Every independently decoded persisted or transmitted Draft contract owns a closed `ContractId`, stable unversioned identifier, and independent current and supported schema-version policy. Every policy currently supports only version `1`. Artifacts remain self-describing through their own `schema_version`; registry metadata validates the expected Rust contract and never reinterprets existing bytes.

A canonical SQLite database, archive, file, envelope, or other independently decoded container owns the version for that boundary. Rows or subrecords that are never encoded outside it inherit the container version. A member that is independently hash-addressed, signed, copied, persisted, transmitted, or decoded is a separate registered boundary with its own metadata. New contracts require a new compile-time `ContractId`; arbitrary runtime registration and string-selected production dispatch are not supported.

## Authority And Caches

The JSON records and the framed Activity Ledger are authoritative. The SQLite database is an index cache and is treated as rebuildable implementation state. Use `draft doctor storage` to inspect storage health.

Losing the index should not lose Draft history. Losing objects or JSON records may corrupt snapshots, evidence, or receipts.

Because the index is derived, a rebuild is allowed to discard it entirely: when the revision recorded in the database is not the one this build writes, `draft maintenance index-rebuild` drops its derived tables and recreates them before re-deriving every row from authoritative state. This is why a rebuild can recover from a schema that no longer matches, and why it never needs a migration.

## Activity Ledger

Draft records what actually happened as an append-only, hash-chained Activity Ledger. Activity is history, not intent: an event is appended once the fact it describes is already durable, which is why the vocabulary avoids names that could outlive the thing they claim.

<!-- retired-architecture-ok: naming what was retired is the point. -->

`.draft/events/events.log` is the **sole authoritative** Activity file, and `.draft/events/events.index` is derived from it and fully rebuildable. There is no `events.jsonl`, no compatibility reader, and no second event stream.

### The physical frame is not the logical record

A stored record is framed — magic, format marker, encoded length, canonical record bytes, checksum, terminator — so that a crash part-way through an append is distinguishable from damage to a record that was committed. The framing bytes are **not** part of event identity: the chain hash and the payload identity are computed over the canonical logical record alone.

That distinction decides what recovery may do:

- a **physically incomplete** final frame — a partial header, a declared length past end-of-file, truncated record bytes or checksum — is truncated back to the last fully verified record, the index is rebuilt, and any undrained audit fact is replayed through the normal idempotent append;
- a **physically complete but invalid** frame, or any corruption inside an earlier record, is never truncated and never replayed over. It goes to `draft doctor` and recovery, because automatically rewriting history is worse than refusing.

### The logical record

- `event_id` — the `evt_` identity, preallocated by the transaction that will append it, so a replayed recovery converges on one record instead of two;
- `previous_hash` — the record this one links onto, or the ledger's genesis hash;
- `record_hash` — the domain-separated hash over `previous_hash`, `event_id` and the canonical payload;
- `payload` — `kind` (the frozen v1 vocabulary name), optional `subject`, `actor`, `recorded_at`, and event-specific `metadata`.

`recorded_at` comes from the fact being recorded rather than from the clock at append time. An append is idempotent on its payload, so a recovery replaying a drain has to produce byte-identical bytes; reading the clock again would not.

### Appending

Appends are serialized by a correctness lock on `.draft/events/events.lock` — an internal lock, never a product lease — so two appenders cannot read the same tail hash and fork the chain. An append with an id already present and an identical payload succeeds without writing a second record; the same id with a different payload is a corruption error.

Exactly one place converts a domain audit fact into an event and appends it (`core::app::activity`). Domain modules and persistence services persist audit facts carrying a preallocated event id; they never construct a payload and never call the ledger. That is what lets the append be the drain step of a durable transaction rather than a side effect somebody remembered.

### The frozen v1 vocabulary

```
ProjectCreated            BaselineInitialized       ProjectClosed
WorkspaceCreated          CheckpointCreated
ProviderSemanticDefinitionAdded   ProviderOperationalProfileAdded
ProviderBindingAdded      ProviderBindingRetargeted ProviderBindingUnbound  ProviderBindingRebound
TaskCreated  TaskUpdated  TaskClosed  TaskReopened
ChangePackCreated  ChangePackDefinitionAmended  ChangePackCompleted  ChangePackAbandoned  ChangePackReopened
AuthorityGranted  AuthorityRevoked  SecurityStateUpdated  PolicyUpdated
OperationPlanned  OperationExecuted  OperationRefused  OperationReplanned
ResourceObserved  CoverageRecorded  RelationDerived  StateBearingDeclared
ScopeResolved  RevisionPackSealed
EvidenceProduced  AssessmentProduced
ReviewSubmitted  DecisionRecorded  GateEvaluated  GateWaived
LeaseAcquired  LeaseReleased  LeaseRefused
PromotionPrepared  PromotionCommitted  PromotionFinalized
PromotionRefused  PromotionAbandoned  PromotionInconsistent   BaselinePromoted
PublicationRequested         PublicationDispatchCommitted
PublicationSucceeded  PublicationFailed  PublicationNoEffect  PublicationIndeterminate
PublicationAbandonedBeforeDispatch
PublicationResolved  PublicationRetryAuthorized  PublicationInconsistent
ReceiptIssued  RecoveryPerformed
ExtensionInstalled  ExtensionAuthorized  ExtensionRevoked
MaintenanceStarted  MaintenanceCompleted  MaintenanceFailed
```

<!-- retired-architecture-ok: naming the rejected event name is the point. -->

The vocabulary is closed and every entry has a named audit-fact owner and a named journal mechanism. There is deliberately **no `PublicationAttempted`**: the durable `Dispatching` boundary happens before Draft invokes a provider, so an event named "attempted" there could outlive a crash in which nothing was sent. `PublicationDispatchCommitted` names exactly what is true at that point.

### Reading and verifying

```bash
draft activity list
draft activity show evt_...
draft activity verify
draft doctor
draft doctor receipts
```

`draft activity verify` verifies the chain. `draft maintenance index-rebuild` is the transactional rebuild of the derived index; it never rewrites the log.

## The machine-scoped audit chain

Acts that belong to the installation rather than to any project — configuring a catalog source, trusting a root, installing or authorizing a package — are recorded in `~/.draft/audit/events.log`. It uses the same framed, locked, hash-chained storage and its own closed vocabulary, and it survives every project being removed.

## Object Store

Objects are addressed by BLAKE3 (`b3:<hash>`). Object bytes are stored compressed with zstd, and `draft maintenance compact` can move loose compressed objects into zstd-compressed segment files with a rebuildable index. The object store is used for workspace file contents, command output, rendered messages, and evidence payloads.

## Privacy

Draft can store file contents and command output. Treat `.draft/` as sensitive project metadata. Do not publish it unless the project intentionally wants to publish review history, evidence, and receipts.
