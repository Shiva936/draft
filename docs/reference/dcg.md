# The Draft Change Graph

Draft's canonical object is the **Draft Change Graph** (DCG). Resources and Relations form the project graph; Changes and ChangeRevisions form the work graph; Evidence, Assessments and immutable Decisions gate a journalled Promotion into an immutable Baseline; Publication to an external provider is a separate, independently identified lifecycle; and an append-only Activity Ledger records what actually happened.

```
Resource / Relation
  → Change → ChangeDefinition → ScopeResolution → ChangeRevision
  → Evidence + Assessment
  → Review + Decision + Gate
  → Promotion  → Baseline  → PromotionReceipt
  → optional Publication → external provider → PublicationReceipt
```

Every arrow in that chain is a _separate_ fact. The separations are the point, and this document is mostly about why each one exists.

---

## Distinctions the model refuses to collapse

| These are different | Because |
| --- | --- |
| observed state / accepted state | Observing a project says what is there; only a Promotion says what the project _accepts_. |
| Change / ChangeRevision | A Change is the work; a revision is one exact sealed state of it. |
| Evidence / Assessment | Evidence says what was observed; an Assessment says what somebody judged it to mean. |
| Assessment / Decision | A judgement of risk is not a decision to proceed. |
| Decision / Promotion | Authorizing work is not accepting it; the commit is a separate durable act. |
| Promotion / Publication | What Draft accepts locally is not what any external system was told. |
| Baseline / provider state | A Baseline is Draft's own accepted history; a provider's state is somewhere else's. |
| planned operation / Activity event | A plan says what was intended; Activity records what happened. |
| local authority / external side effect | Draft's history is authoritative locally; an external provider is optional and subordinate. |

---

## The exact-reference rule

Use a bare id for navigation. Use **id + digest** when substituting the referenced immutable bytes could change accepted provenance, security authority, external-side-effect meaning, or historical verification.

The v1 exact references are exactly these — the audited set:

| Reference | Fields | Where it appears |
| --- | --- | --- |
| `ObservationRef` | `{ id, digest }` | state evidence, relation provenance, Evidence and Assessment inputs, sealed revision observations, scope/impact/proof facts, DraftPack references, receipt payloads |
| `ObservationRunRef` | `{ id, digest }` | `Observation.run`, `CoverageEvidence.observation_run` |
| `SecurityFactRef` | `{ kind, logical_id, digest }` | every immutable authorization-bearing fact — `StateBearingDeclaration.authorizing_grant`, `GateWaiver.authority`, and the gate security context |
| `PublicationRef` | `{ id, digest }` | `PublicationRetryAuthorization.publication`, publication registry entries |
| `PublicationAttemptRef` | `{ id, digest }` | attempt-bound facts and GC reachability |

**At every one of those boundaries:** load by logical id → recompute the canonical digest → require equality. A mismatch is `CorruptData` / `IntegrityViolation`. It is never a warning, and it is never repaired in place.

Anything not in this table is deliberately a bare id. Adding a digest where substitution cannot change meaning costs nothing but makes it harder to see which references actually carry integrity weight.

---

## Immutable-fact integrity

Every immutable fact is stored **create-once**, bound to the digest of its own canonical bytes:

```
LogicalId  →  CanonicalPayloadDigest        created once, verified on every load
same id + byte-identical payload            idempotent success
same id + different payload                 integrity failure, never an overwrite
```

This is what makes "the same revision" mean _the same bytes_ rather than the same string. An approval, a gate evaluation or a promotion that cites `rev_abc` cannot be made to describe different content by rewriting what `rev_abc` stores — the binding is checked when the fact is loaded, before anything consumes it.

The rule is universal. `ChangeDefinition`, `ScopeResolution`, sealed `ChangeRevision`, `Operation`, `Evidence`, `Assessment`, `ChangeRepresentation`, `Review`, `Decision`, `GateWaiver`, `GateEvaluation`, `PromotionRecord`, `Publication`, `PublicationAttempt`, `PublicationOutcome`, `PublicationResolution`, `PublicationRetryAuthorization` and every receipt envelope are stored this way.

### Nothing carries across revisions

Evidence and Assessments bind one exact `ChangeRevisionId` and there is deliberately no way to ask whether they also cover a later one. The tempting shortcut — the tests passed, the author edited one file, surely the result still holds — is exactly the case nobody has actually checked. An approval resting on evidence gathered before an edit is an approval of work that was never examined.

---

## Provenance is not a route

Two provider values look similar and mean entirely different things:

```
ProviderProvenanceRef { binding, semantic_definition }
    — WHAT HAPPENED. Immutable. Part of accepted history.

ProviderRouteRef { provenance, operational_profile }
    — WHERE WORK WOULD GO. Constructed at planning time. Never a Baseline field.
```

**A Baseline composition exposes `accepted_provider_provenance`, never a route.** A Baseline says which binding and which semantic definition produced the primary Observation for each accepted Resource state. It says nothing about where new work would be sent, because that is a question about now.

A `ProviderRouteRef` is constructed only when executable routing is planned, and the resulting immutable plan **never resolves current pointers at execution time**:

```
planned route = SD1 / OP1;  the binding later advances to SD2 / OP2
→ the existing Operation or Publication remains SD1 / OP1
→ the plan is now STALE: refuse / re-plan / re-authorize / republish
→ never silently adopt SD2 / OP2
→ never execute SD1 / OP1 as a "still usable" historical route
```

Before the first provider side effect, execution takes the binding's correctness lock and requires **exact equality** with the binding's current pointers — lifecycle `Active`, the same binding id, the same current semantic definition, the same current operational profile. Availability is never sufficient authority to execute. A historical definition remains loadable for verification, historical reads, GC reachability and explicit recovery, and for nothing else.

`unbind` deactivates and destroys nothing. For an `Unbound` binding, historical verification, reads, GC reachability and explicit recovery remain allowed; new observations, routing, mutations, Publication and materialization are refused. `rebind` is the explicit reactivation.

### What this means for Baseline identity

Changing only the current operational profile `OP1 → OP2` leaves `BaselineId`, `StateEvidenceRoot` and the historical Baseline composition **unchanged**, and the accepted provenance stays `{ pbd_A, SD1 }`. A newly planned Operation explicitly constructs `ProviderRouteRef { pbd_A, SD1, OP2 }`. Historical projections are never invalidated by current provider state.

---

## Baseline identity

```
BaselineManifest {
    project, project_state_root, state_evidence_root, coverage_evidence_root,
    parent_baseline_id: Option<BaselineId>, format_revision: 1
}  →  BaselineId
```

| Root                   | Answers                                                                     |
| ---------------------- | --------------------------------------------------------------------------- |
| `ProjectStateRoot`     | what material state is accepted                                             |
| `StateEvidenceRoot`    | what exact provenance establishes the state entries that exist              |
| `CoverageEvidenceRoot` | what exact coverage claims justify absence and domain completeness          |
| `BaselineId`           | the exact accepted historical node: state + provenance + coverage + lineage |

### Timestamps: the rule is reachability, not datatype

Never say "timestamps are excluded from `BaselineId`". Three separate statements are true, and only all three together are accurate.

**Material state identity.** `ProjectStateRoot` excludes observation and provenance timing. Changing only `Observation.observed_at`, `ObservationRun.started_at` / `completed_at`, or `StateBearingDeclaration.declared_at` never changes `ResourceStateDigest` or `RelationStateDigest` merely because time moved.

**Accepted historical-node identity.** Those provenance timestamps live _inside_ canonical provenance objects whose digests feed the evidence and coverage roots. So a changed provenance object — including its timestamp fields — may produce a different evidence or coverage root and therefore a different `BaselineId`. Two Observations of identical material state differing only in `observed_at` give the **same** `ProjectStateRoot`, a **different** `ObservationDigest` and `StateEvidenceRoot`, and therefore a **different** `BaselineId`. Provenance timestamps are never stripped from those objects to preserve a tidier sentence.

**Genuinely excluded.** Only metadata outside `BaselineManifest` and the canonical roots it references: `BaselineRecord.accepted_at`, `BaselineRecord.actor` and display metadata, Activity append timing, telemetry timestamps, Publication runtime timing, non-manifest receipt and event identifiers, publication status, promotion-attempt metadata, credential material, the provider operational profile, and other envelope metadata unreachable from the manifest roots.

### Absence has to be proved

A Resource with no established state does not appear in `ProjectStateRoot`. Its absence is justified by `CoverageEvidenceRoot`, which distinguishes _not observed because nothing was attempted_ from _not observed because the attempt failed_:

| `status` | `attempted` | `committed` | `observation_run` | `known_gaps` |
| --- | --- | --- | --- | --- |
| `Complete` | `true` | `true` | `Some(..)` | empty for the claimed domain |
| `Incomplete` | `true` | per the repository model | `Some(..)` | non-empty |
| `NotObserved`, nothing attempted | `false` | `false` | **`None`** | may be empty |
| `NotObserved`, attempt failed | `true` | `false` | `Some(..)` | may carry the failure gap |

Any other combination is a hard construction error. `attempted == false` with `Some(run)` is rejected: there is no synthetic run for an attempt that never happened. `attempted ≠ committed` is preserved, so a failed attempt can never be promoted to `Complete`.

Stronger coverage over identical material state yields the same `ProjectStateRoot`, a different `CoverageEvidenceRoot`, and therefore a different `BaselineId`. That is deliberate: the project accepts a different historical node because it knows more about it.

**No user-facing surface may imply that an empty Resource set proves complete observation.**

---

## Staged versus dispatched publication attempts

A publication attempt has two distinct existences, and conflating them is how a system reports an external effect that never occurred.

```
STAGED      the attempt's immutable artifact is written and its number allocated
            → nothing external has been contacted
            → no primary outcome is required, ever
            → it is GC-eligible once journal and control recovery complete

DISPATCHED  the journal durably reached Dispatching
            → Draft committed the authority and state permitting this exact
              external dispatch
            → PublicationDispatchCommitted is appended
            → EXACTLY ONE primary outcome must eventually exist
```

<!-- retired-architecture-ok: naming the rejected event name is the point. -->

There is deliberately **no `PublicationAttempted` event**. The durable `Dispatching` boundary happens _before_ Draft invokes the provider, so an event named "attempted" emitted there could outlive a crash in which the provider was never called. `PublicationDispatchCommitted` names exactly what is true at that point: Draft durably committed the authority and state required to permit this exact external dispatch. It does not assert that the provider received anything. The outcome events record what reconciliation actually established.

Status wording follows the same rule. A dispatch that was authorized and durably committed but whose result is not yet known reads as _dispatch committed, result pending reconciliation_ — never as "attempted and succeeded/failed".

### A valid digest is not validity

A digest proves the stored bytes have not been edited since they were written. It proves nothing about whether they were coherent when they _were_. Every Publication-family object therefore recomputes its own derived fields and its cross-object references on construction, on parse and on verification:

| Object | What is recomputed or required |
| --- | --- |
| `Publication` | `request_key` and `idempotency_key`, from its own canonical inputs |
| `PublicationAttempt` | the exact `PublicationRef`; `route == publication.route`; the number the allocation reserved; the exact dispatch security snapshot |
| `PublicationOutcome` | the exact `PublicationAttemptRef` whose head it is filed under; the preallocated `ReceiptId` and frozen signer binding |
| `PublicationResolution` | the exact `PublicationOutcomeDigest` whose head it advances; a supersession chain that never crosses outcomes |
| `PublicationRetryAuthorization` | the exact `PublicationRef`; a `prior_outcome` belonging to an attempt of _that_ Publication; `duplicate_risk_acknowledged` and `authorizes_one_attempt` |
| `PublicationControl` | its own `publication_id`; every consumed authorization belonging to that Publication |

An object whose derived fields disagree with its own inputs is invalid **even when its outer digest matches its (self-consistently corrupted) bytes**. The committed vectors under `proto/test-vectors/` carry exactly such payloads, so an external implementation can check its rejection path against Draft's rather than discovering the gap in production.

### Allocation, abandonment and attempt numbers

An attempt number is read and committed only inside `PublicationControl`'s serialized critical section, so two contenders can never both obtain `N`.

| Situation | Number consumed? | Terminal state |
| --- | --- | --- |
| `AttemptPrepared`, control never committed | **no** | `Abandoned` — terminal, no `Finalized` |
| allocation committed, refused before dispatch | **yes** | `AbandonPrepared → AbandonedBeforeDispatch → Finalized` |
| dispatched, outcome recorded | yes | `OutcomeRecorded → Finalized` |

An uncommitted candidate number creates no gap, so a later `pat_B` may legitimately become the authoritative attempt `N`. A committed allocation that was later abandoned pre-dispatch is the only legitimate source of a gap.

`Abandoned` is a historical proof that one candidate allocation never committed. It carries its own classification evidence, it does not freeze `PublicationControl` forever, it stays valid beside later legitimate Publication activity, and it can never become authoritative. Current control referencing an `Abandoned` attempt, or a primary outcome existing for one, is a hard consistency violation.

---

## Representations, impact and proof coverage

Three derived answers about a sealed revision, each deliberately weaker than it could be.

### Representation — what the revision did

A `ChangeRepresentation` explains one Resource's transition, and a bundle of them binds one exact `ChangeRevisionId`. It is recorded when the revision is sealed, from the same observations the revision was sealed over: deriving it later would explain a workspace that has since moved.

Each touched Resource resolves to a strategy — a contributed presentation where one claims the Resource by specificity, and otherwise the **neutral rendering**, which always exists and no extension contributes. The neutral rendering says exactly what Core can justify: which Resource changed, between which two authoritative state digests, and a `Whole` conflict claim, because Draft cannot say where inside a Resource the work landed.

It does not diff content, parse a payload or interpret a domain. Doing any of that would make Core the semantic authority for every kind of Resource, which is the coupling the whole contribution model exists to avoid. `provenance` is a sum — `Core` or `Extension` — precisely so a contributed producer that generates its own payload records its own provenance rather than Core pretending to be the author of bytes it wrote.

Representations are what let composition give a finer answer than whole-Resource overlap: two revisions whose claims are provably disjoint compose, and two with no representations at all do not.

There is deliberately **no representation Activity event**. The v1 vocabulary is closed, and a representation is a regenerable explanation of sealed work rather than a decision anybody took. It is a create-once immutable fact and a GC root, and that is all.

### Impact — what the revision reaches

Elements inside Resources, and the Resources related to them. An element has a stable id, an optional namespaced kind, a name and typed attributes; a relation has a namespaced kind and two endpoints. Core stores, links and counts them and never learns what any of them means — there is no built-in notion of a "public API" or a "reference".

Extraction is contributed and authorized. An element exists because an extractor said so; a relation exists because one said so. Directory layout, dependency edges, graph proximity and name similarity produce **no** elements. Resources nothing installed can extract from are reported as `unextractable`, because without that "no elements" would mean both "nothing is in there" and "nothing knows how to look".

### Proof coverage — what the evidence speaks for

```
same directory                  → NOT coverage. Layout is a filing habit.
imported / depended upon        → NOT coverage. Using a thing does not test it.
adjacent in the graph           → NOT coverage. An edge is not an assertion.
reachable from something tested → NOT coverage. Reachability is not exercise.
named similarly                 → NOT coverage. Names are a convention.
```

Each of those is a plausible heuristic, and each would produce a confident `covered` for a Resource nothing has ever verified. The distance between "related to something tested" and "tested" is the distance between a passing review and an outage.

Two things count. **Direct** coverage requires evidence that read an observation of that exact Resource. **Indirect** requires somebody to have said so — a declared coverage relationship in the graph, or an attestation from the producer that generated the evidence — followed exactly one hop, so every indirect answer traces to a single assertion a person can read and disagree with.

Indirect is reported as its own answer rather than folded into `covered`, and each indirect source reports its own availability: "no declared relationship asserts this" and "Draft has nowhere to record such an assertion" are different facts, and v1 records neither kind of assertion.

---

## The Activity Ledger

Activity is history, not intent. See [Storage And Events](../internals/storage-and-events.md) for the physical format, the framing rules, the frozen v1 vocabulary and the append discipline.

Two properties matter here:

- **Every named event has a durable audit-fact owner and a named journal mechanism.** No authoritative fact commits before the exact audit fact and outbox material describing that commit is durable.
- **Exactly one place converts a domain fact into an event and appends it.** Lower layers persist audit facts carrying a preallocated event id; they never construct a payload and never call the ledger.

---

## Receipts

A v1 receipt attests exactly one of three things: a Promotion that accepted a Baseline, a publication attempt's primary outcome, or an authorized Resolution of one. Local actions that are already immutable facts in their own stores — a checkpoint, a decision, a sealed revision — are not receipted, because two records of one act with no rule for which is authoritative is worse than one.

Verification reports three levels separately, and what cannot be determined reads `unknown` — never `valid`:

```
Structure / canonical form   do the stored bytes re-derive the signed message?
Signature                    does the signature cover exactly this payload and binding?
Historical trust             was the key accepted when the receipt was issued?
Current trust                is the key accepted now?
```

"Signature valid, key since revoked" is a real and important state. Collapsing these into a single yes/no is how it becomes invisible.
