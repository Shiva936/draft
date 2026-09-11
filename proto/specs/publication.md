# Publication Protocol

Publication delivers an accepted Baseline to an external provider. It is a
separate, independently identified, multi-target, independently journalled
lifecycle. Nothing in it can change what the project accepts: an unreachable
provider cannot invalidate accepted history, and a successful delivery does not
make anything authoritative that a Promotion did not already accept.

External providers are optional throughout. A project with no binding publishes
nothing and loses no capability.

## Objects

```
Publication            the immutable intent to deliver one exact Baseline to one exact route
PublicationAttempt     one allocation of that intent, numbered inside PublicationControl
PublicationOutcome     the primary result of one attempt — AT MOST ONE per attempt id
PublicationResolution  an authorized later interpretation of one exact outcome
PublicationRetryAuthorization
                       a one-shot permission fact bound to an exact PublicationRef
```

`Publication.request_key` and `Publication.idempotency_key` are derived from the
object's own canonical inputs and are recomputed on load. Bytes carrying a valid
outer `PublicationDigest` over derived fields that disagree with their canonical
inputs are still rejected — the outer digest proves the bytes are unchanged, not
that they are self-consistent. The same rule applies to
`PublicationAttempt.route != Publication.route`, an outcome whose `attempt`
differs from its head's `pat_`, a Resolution superseding a resolution for
another outcome, and a retry authorization whose `prior_outcome` belongs to
another Publication.

No canonical Publication object contains a `CredentialHandleRef` or any secret.
A credential handle is pure secret indirection: it may resolve a secret and
nothing else. A replacement handle is acceptable only if it satisfies the same
frozen non-secret account, tenant and authority semantics; one resolving to
another account or tenant is rejected and cannot redirect the Publication.

## Stores and their locks

| Store | Lock | Owns |
|---|---|---|
| `PublicationRegistryStore` | `publication/registry.lock` | `PublicationRequestKey → PublicationRef`, unique |
| `PublicationJournalStore` | `publication/journal/<pat_>.lock` | every per-attempt state transition |
| `PublicationControlStore` | `publication/control/<pub_>.lock` | in-flight attempt, next attempt number, consumed retry authorizations |
| `PublicationOutcomeStore` | `publication/outcome-heads/<pat_>.lock` | at most one primary outcome per attempt |
| `PublicationResolutionStore` | `publication/resolution-heads/<outcome-digest>.lock` | the active resolution per outcome |
| `PublicationRetryAuthorizationStore` | `publication/retry-authorizations/<pub_>.lock` | retry-authorization **facts** |

Every per-attempt transition goes through `transition_locked`. There is no
last-writer-wins path to `Dispatching`, `OutcomePrepared` or `OutcomeRecorded`,
and no journal record is written outside its owning store.

**No lock — journal, binding, control or lease — is held across the external
provider call.**

## Attempt numbers

An attempt number is read and committed **only** inside `PublicationControl`'s
serialized critical section. Two legitimate contenders can therefore never both
observe the same `next_attempt_number`: the second blocks on the control lock,
re-reads the committed state, sees the in-flight attempt, and is refused.

A `candidate_attempt_number` exists purely for the window between persisting
`AttemptPrepared` and committing the control mutation.

```
allocation never committed   → number NOT consumed, no gap, journal reaches Abandoned
allocation committed         → number consumed; an abandonment after it is the
                               only legitimate source of a gap
```

So a later `pat_B` may legitimately become the authoritative attempt `N` that an
uncommitted `pat_A` once carried as a candidate.

## Retry authorization

Creation, current dispatch eligibility, and one-shot consumption are three
different things.

- **Creation** requires current authority and is journalled by
  `PublicationRetryAuthorizationStore`. That lock owns no consumption state.
- **Consumption** is an atomic check-and-insert into
  `PublicationControl.consumed_retry_authorizations`, in the same transaction as
  the attempt allocation. There is no second consumption bit anywhere.
- **Eligibility at dispatch** is evaluated fresh. A historical authorization
  fact is not a current permission to mutate externally: a later dispatch
  performs its normal current-security validation and may refuse.

## The per-Publication bookkeeping barrier

Before any new allocation, Phase 0 classifies the Publication's local
bookkeeping. It uses one state-specific recovery lockset at a time and **ends
holding nothing — no lock, no lease, no fence** — so Phase 1 can take
`TrustReadFence` first and then a fresh `PublicationLease`.

**Phase 0 is a classifier and a local finalizer, not the executor of every
recovery step.** It finishes everything it can finish locally. When an
already-allocated attempt needs current authority, security or provider-route
validation, Phase 0 hands off and ends: that validation runs only after every
Phase-0 lock and lease is released. This is what keeps the barrier local-first —
it never acquires `TrustReadFence`, `ProjectControlStore` or
`ProviderBindingStore`, and never waits on a provider.

**A state-specific recovery lockset** is the complete set of locks that *one*
item's current authoritative state requires, held together and released in full
before the next item is processed. The frozen shapes are `3 → 6`, `3 → 6 → 7`
and `3 → 6 → 9` — a lease and a journal, optionally reaching the control record
or one group-9 lock. Never one giant lockset spanning several items, and never
two group-9 locks at once.

The barrier returns an explicit result, and exactly one value permits a new
allocation:

```
Clean                        no unresolved local bookkeeping AND
                             PublicationControl.in_flight_attempt == None
RecoverAllocatedAttempt(pat) a committed AttemptPrepared resumes THAT attempt
PendingExternalResolution    an unresolved external effect; non-blocking locally,
                             but no further external attempt for this Publication
```

A missing or unreadable journal beside an in-flight reference is never `Clean`.
No attempt-local identity — no `pat_`, receipt id, signer binding or event id —
is minted before `Clean`. Recovery never falls through into allocation: a new
attempt requires restarting from Phase 0 once the previous attempt is locally
terminal. `Clean` never replaces Phase 2's authoritative re-read of
`PublicationControl` under its own lock.

## Dispatch

```
Phase 1   optional TrustReadFence, then a fresh PublicationLease
Phase 2   PublicationControl: validate, allocate, consume any PRA — atomically
Phase 3   ProviderBinding: require EXACT equality with current pointers
Phase 4   journal AttemptPrepared → Dispatching (PublicationDispatchCommitted durable)
          release every lock
Phase 5   invoke the provider
Phase 6   deterministic re-read; OutcomePrepared → record_once → OutcomeRecorded
```

Before the first provider side effect, the exact binding's correctness lock is
held and exact equality is required: lifecycle `Active`, the same binding id,
the same current semantic definition, the same current operational profile, and
credential-authority requirements still satisfied. A still-existing older
definition is never sufficient.

`PublicationDispatchCommitted` is emitted only at the durable `Dispatching`
transition, and its audit fact is durable in that same transition. It asserts
that Draft committed the authority and state permitting this exact dispatch —
not that the provider received anything.

## Outcomes

Every primary outcome, from every source, passes through
`prepare_and_record_primary_outcome`. No path calls `record_once` directly from
`Dispatching`.

```
Dispatching → OutcomePrepared(candidate + exact result AuditFacts)
            → PublicationOutcomeStore::record_once
            → OutcomeRecorded
```

The immutable outcome object is written and fsynced **before** its head is
committed, so a head never resolves to a missing or unverifiable object.

Journal serialization makes candidate selection unique: whichever writer takes
the journal guard first persists the only `OutcomePrepared` candidate, and the
other observes that candidate and may not prepare a different one. A
different-candidate `record_once` `Conflict` is therefore unreachable during
legitimate execution, and is classified as an **integrity violation** routed to
Doctor and recovery — never as ordinary worker concurrency.

- At most one primary outcome per `PublicationAttemptId`.
- Exactly one eventually, for every attempt that durably reached `Dispatching`.
- A staged, undispatched attempt needs none, ever.
- `Dispatching` with an existing primary outcome is a hard integrity failure,
  never a fast-forward. `OutcomePrepared` with the primary outcome equal to the
  candidate is the legitimate neighbouring state, and does fast-forward.
- `OutcomeRecorded` for `pat_A` while `PublicationControl` already references
  `pat_B` is a hard consistency violation. Recovery never clears, mutates,
  finalizes or otherwise touches `pat_B`; it routes to Doctor and recovery. It
  is distinct from the valid case of a `Finalized` historical `pat_A` beside
  legitimate later state for `pat_B`.

## Late provider results

A result that arrives after a primary outcome was established is **operational
reconciliation input**. It is never a second primary candidate, never an unowned
canonical fact, and never a cause of journal regression. It becomes
authoritative only through a `PublicationResolution` committed under **current**
resolution authority.

## Terminal dispositions

```
Abandoned                    an allocation proven never committed.
                             TERMINAL. No Finalized transition. No audit fact to
                             drain. Retains its exact classification evidence.
AbandonedBeforeDispatch      a committed allocation refused before dispatch.
                             AbandonPrepared → exact whole-value control clear →
                             AbandonedBeforeDispatch → audit drain → Finalized.
Finalized                    durably preserves or verifiably references its
                             terminal disposition, so an outcome-path attempt
                             must have exactly one primary outcome and an
                             abandonment-path attempt must have none.
```

`AttemptPrepared + exact_expected` proves non-commit **at classification time**.
It is not a perpetual postcondition on the mutable `PublicationControl`: later
legitimate `pat_B` activity leaves a historical `Abandoned` `pat_A` valid.

AR0 classifies control separately, then **reacquires the journal and re-requires
the same exact `AttemptPrepared`** before the guarded `AttemptPrepared →
Abandoned` transition. A crash between the two simply reruns AR0; no stale
classification is trusted.

`Abandoned` is never a bare marker. It persists which `pat_` was classified, its
candidate number, the exact expected and planned control states, which retry
authorization the planned allocation would have consumed, and enough of the
allocation inputs to prove that observing `exact_expected` meant the allocation
never became authoritative. The record carries its own proof rather than a
pointer whose later value would have to stay unchanged.

### Guarded reads are not a held guard

Every authoritative journal read and every journal transition happens under the
journal guard. That is **not** the same as holding one guard across a whole
multi-phase recovery workflow, and where a workflow would otherwise hold the
journal (order 6) and reach for order 1, 3, 4 or 5 it **must** release and
reacquire instead.

So committed-`AttemptPrepared` recovery releases its Phase-0 state *before*
current-authority validation:

```
AR0   journal(6) read+release; control(7) read+release; classify
      → uncommitted: reacquire journal(6) independently, transition to Abandoned
      → committed:   hand off; Phase 0 ends holding nothing
AR1   optional TrustReadFence(1) → PublicationLease(3) → project control(4)
      → provider binding(5)                     no journal or control lock held
      → then reacquire journal(6) and control(7), re-read, re-require the exact
        expected values, and only then transition
```

**Every phase boundary re-reads the authoritative journal and control values
before any mutation.** If either moved, the stale validation is discarded, the
guards are released, and the workflow reclassifies from authoritative state.

### Pre-dispatch abandonment has exactly one shape

There is **one** frozen abandonment transaction, and no alternative:

```
AV   optional TrustReadFence(1) → PublicationLease(3) → project control(4)
     → provider binding(5)        no journal or control lock held
     re-read and verify project lifecycle, security state, policy, authority,
     the exact PublicationRef and ProviderRouteRef, binding lifecycle, current
     definition, current profile, credential-authority requirements
AC   THE SAME AV GUARDS REMAIN HELD, then journal(6) → control(7):
     require the exact AttemptPrepared, require the exact committed in-flight
     control state, persist AbandonPrepared, fenced whole-value control clear,
     AbandonedBeforeDispatch, release in strict reverse order
```

AC never releases the AV guards and revalidates under a different security
snapshot: the refusal snapshot recorded in `AbandonPrepared` is precisely the
one validated while those exact guards were held. The terminal
`AbandonedBeforeDispatch → Finalized` transition later reacquires only the
journal lock, after the audit fact has drained exactly once.

## Recovery is not authorization

Recovering an already-committed Resolution or retry authorization does **not**
require current authority — it finalizes from the fact's own frozen commit-time
snapshot (`AuthorityDecision`, `ProjectSecurityStateDigest`, policy digest,
registry revisions). Creating a **new** one does.

Both the Resolution and the retry-authorization transactions re-read their
journal after acquiring the current-security guards. If a transaction appeared
in between, the actor releases both guards and restarts from the recovery phase
rather than recovering somebody else's transaction while holding
`ProjectControl` or `TrustReadFence`. No journal is overwritten and no second
fact is minted.

## Remote unavailability

Recovery never waits indefinitely for a provider and never holds a lock or lease
while waiting. Unrelated local Draft work always proceeds, and so does an
unrelated Publication.

Non-blocking is **not** permission to issue a duplicate external mutation. Where
a prior external effect cannot yet be classified safely, Draft conservatively
prevents another external attempt for that **same** Publication until recovery
establishes a safe state.

## Surfaces

```
draft baseline publish list <bas_...>
draft baseline publish retry <pub_...>                  requires CURRENT authority
draft baseline publish resolve <pub_...> --outcome <digest>
    --mark-succeeded <external-ref> | --mark-failed
draft baseline publish authorize-retry <pub_...> --acknowledge-duplicate-risk
draft baseline republish <bas_...> --target <pbd_> [--purpose <id>] --republish-intent <id>
```

Status wording distinguishes dispatch from result. A dispatch that was
authorized and durably committed but whose outcome is not yet known reads as
*dispatch committed, result pending reconciliation* — never "attempted and
succeeded/failed". An `Indeterminate` outcome offers **Resolve** and **Authorize
retry**, never a plain retry.
