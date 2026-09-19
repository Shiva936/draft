# Architecture

Draft is structured as a local-first workspace built in Rust:

```text
cli                 Console web compatibility API
 |                              ↑
 |                  draftd authoritative Console models
 |                              ↓
 |                  console/application → console/tui
 |                              |
 +-------------------------- draftd
    |
draft-core::App
    |
global + project .draft/ durable stores
```

## Crate Boundaries

`sdk/dcg-contract/` is the portable canonical boundary: the identifier grammar, the Change Graph's canonical values and digests, the Publication family, and receipt payloads. It depends on **no** Draft crate at all — a verifier can check a Draft receipt or reconstruct a Baseline identity without linking Core, a service, or the CLI.

`sdk/extension-contract/` builds on it: the declarative package format and signed catalog format. It lets extension authors, publishers and tools build against Draft without building Draft. Libraries under `sdk/` are externally consumable and dependency-light, but that location alone does not promise permanent semver/API stability while Draft remains pre-release.

`core/` owns the domain model and durable stores. It implements config, observation, snapshots, tasks, executions, ChangePacks and revisions, import and export, evidence, assessment, gates, decisions, policy, promotion, publication, signed receipts, recovery anchors, the Activity Ledger, transparency, object storage, the impact index, and indexing. Its `extension` domain owns extension semantics and authorization state; it holds no knowledge of any particular domain, language, ecosystem or toolchain, and reads contributed knowledge through a single port.

`cli/` exposes the command-line interface. It invokes `draft-core` directly so the CLI stays usable without a daemon.

`console/` owns the browser Console boundary. Its Rust crate provides loopback HTTP transport, browser/session security, daemon integration, and embedded assets.

- `console/web/` owns React presentation source
- `console/dist/` is the committed reproducible build. Console does not own Draft domain behavior.
- `console/application/` is the typed Rust client for the separately versioned Console application protocol.
- `console/tui/` owns reducer state, navigation, terminal rendering, and local interaction only. Neither `console/application/` nor `console/tui/` crate depends on `draft-core`, reads project state, or computes lifecycle/action eligibility.

`services/` contains optional local services:

- `draftd`: IPC dispatcher and control plane;
- `extension-service`: extension sources, signed catalog trust, discovery, package acquisition, validation, installation, installed state, and durable capability authorization;
- `ipc`: local request/response transport;
- `watcher`: debounced workspace notifications with Draft write-back filtering;
- `store`: local service registry records;
- `locks`: cross-platform local locks;
- `sessions`: connected-client accounting;
- `sync`: reserved no-network boundary for later design work.

### Extensions

Draft owns the extension platform; `/extensions/` owns individual first-party capabilities.

`/extensions/` is a separate Cargo workspace holding declarative package sources, a packaging tool and standalone conformance tests. It depends only on the published `draft-extension-contract` crate, never on Draft's core, services, daemon or stores, and the root workspace excludes it — so the platform builds and passes its tests with the directory deleted. Moving those sources into their own repository is a build and release change, not an implementation change.

Contributed knowledge flows one way:

```text
declarative package
 → validated contribution
 → extension contracts
 → draftd / Draft-owned interpreter
 → Draft's own hardened execution and evidence paths
```

Four decisions stay separate, and none implies the next: trusting a source, installing a package, authorizing a capability, and running a declared command. A grant binds to one exact artifact — id, source, publisher, version and content digest — so any update needs authorizing again. Draft ships no extension packages and installs none on first run.

Contributions resolve by meaning, never by order. Candidates are ordered by extension id so diagnostics and tests are reproducible; that order never selects a winner.

What composition means is defined per contribution kind, because the kinds genuinely differ:

| Kind | Composition | Why |
| --- | --- | --- |
| `resource_adapter` | unique per scheme | two adapters owning one scheme is an ambiguity, not a merge |
| `resource_classification` | keyed union by `(resource, class)` | a resource is legitimately several things at once |
| `comparison` | unique per resource | one primary explanation of how something changed |
| `element_extraction` | keyed union by extractor id | complementary extractors compose |
| `presentation` | specificity; ties are ambiguous | never an arbitrary winner |
| `tool_action` | keyed union by action id | independent actions coexist |
| `verification` | keyed union by check id, then the state lattice | independently named checks all run |
| `risk_rule` | aggregate | weights sum |
| `policy_preset` | conservative merge per half | the stricter reading wins |
| `intent_vocabulary`, `task_template`, `candidate_preset` | keyed union by id | independent vocabulary |
| `documentation` | aggregate | metadata only |

Classification is the case worth stating plainly, because it used to be modelled as a conflict. Two publishers that both recognize a Rust file — one as a text document, one as a language source — are stating **two facts**, and both are kept. Only two incompatible definitions of the _same_ class collide, and that collision is scoped to that class: every other assignment on the resource still stands.

Whether a contribution kind can be consumed at all is a separate question from whether the contract defines it. `draft-extension-contract` accepts the whole published vocabulary, so an author may target a Draft newer than the one in hand; installation refuses a package whose declared contribution this build has no subsystem for, naming it, before anything is recorded as installed or enabled. Nothing is ever validated, installed and then silently ignored.

## The Governing Rule

> **Draft owns mechanism, authoritative observation and change identity, safety, authority, provenance, orchestration, validation, acceptance and recovery invariants. Extensions own domain interpretation, resource backends, classification, representations, extraction, presentation bindings, transformation semantics, verification semantics, risk semantics, policy vocabulary, task and candidate strategy, and domain tooling.**

Everything below follows from that division. Where the two are confused, the platform stops being domain-neutral.

## The Layers

```text
AUTHORITATIVE PROJECT TRUTH
    ObservationContext
        AdapterObservationBindings   — effective observer mechanism
        ViewRuleBindings             — declarative observation semantics
            ↓
    Snapshot
        RawResourceState (mandatory state_digest)
        scoped opaque CoverageDomainRefs
        observation map / gaps
            ↓
    Candidate / Operations
            ↓
    Snapshot
            ↓
    ChangeSet — proved changes, plus explicit derivation uncertainty

DERIVED INTERPRETATION (never changes the above)
    ClassificationBundle · RevisionPackRepresentationBundle · ImpactIndex
    VerificationEvidence · RiskAssessment · Presentation

HUMAN ACCEPTANCE
    AcceptanceContext → Review / Approval / Waiver → Receipt

RECOVERY
    RecoveryAnchor (live-fenced, per-resource, target-bound)
        → Draft-authored restore → re-observation → RollbackOutcome
```

Two control contexts are kept apart and never mixed: **ObservationContext** (what Draft could see, and by exactly which mechanism) and **AcceptanceContext** (what Draft required before allowing a transition to be accepted). A policy change re-evaluates readiness; it never invalidates a candidate. An observation-semantics change re-baselines; it never manufactures a change set.

Four statements are worth keeping in mind, because most subtle errors are a violation of one of them:

- **A classification change means Draft learned something new about unchanged state.** It is not itself a project change.
- **Absence must be proved, never inferred from an incomplete observation.** A resource missing from a scan that failed is uncertain, not deleted.
- **Extensions propose effects; Draft authors authority-bearing operations.**
- **Observable is not restorable, and observing the present never reconstructs the past.**

## Contributed Identifiers Belong To Whoever Minted Them

Every identifier an extension mints — a resource class, a comparison strategy, an extractor, a presentation, a tool action, a verification check, a risk rule code, an intent, a task template, a candidate preset — is namespaced to the extension that declares it, and that ownership is checked twice: when the package is built, and again when its contributions are resolved. A package claiming an id outside its own namespace contributes nothing from it.

One consequence is worth stating plainly, because it changes what "disagreement" can mean. A class id has exactly one publisher, so two publishers can never dispute one — the composition Draft has to arbitrate is not "who is right about `x/source`" but "this resource is several things at once", which is a union and not a conflict. The collision case that remains is a package that declares the same id twice and means different things by it, and that stays scoped to the disputed id: everything else the package contributes still stands.

## Policy Has Two Halves, And They Never Mix

A contributed `policy_preset` is split, and the split is not cosmetic.

**`view_rules`** decide what is part of the observed universe at all. They therefore participate in the `ObservationContext`, and through it in snapshot identity: adopting a new view rule is a re-observation with a new baseline, never a project change. Draft contributes none. That an external history tool's control directory, or a dependency cache, is not authored project state is a judgement about the software domain; it arrives from `draft.software.project`, and with nothing installed those directories are ordinary project state.

**`control_policy`** decides what Draft requires before accepting a change — protections, risk thresholds, reviewability budgets, verification escalations. It participates only in the `AcceptanceContext` and can never manufacture, invalidate, or reshape a change.

Two consequences follow, and both are enforced rather than merely intended. The canonical workspace view and the authoritative snapshot apply the _same_ contributed exclusions, so an excluded resource changing can never look like a reason to re-verify. And Draft owns exactly one protection — `.draft/**`, structurally, ahead of any rule list — while credentials, key material and registry tokens are protected by an installed preset or the project's own config, composed as a union that no later layer can relax.

## Three Kinds Of Extensibility, And Only One Touches The Model

Adding a _domain_ is model work an extension does declaratively: it contributes classes, comparison, extraction, verification, risk, policy and presentation bindings, and Draft's resource, change, authority and acceptance contracts do not change to admit it. That is the property invariant 32 states — a new domain never requires a Core schema change.

Adding a _presentation binding_ is not model work at all. A binding says which of the platform's rendering engines a resource should be shown through, and with what configuration. It is selected by specificity, ties are reported rather than settled, and where nothing binds the universal neutral presentation always applies. Presentation never affects verification, risk, conflict, review-gate or submission semantics, so a project that installs a renderer preference has changed nothing about what its changes _are_.

Adding a _native renderer_ is platform work. The set of presentation engines — `metadata_summary`, `byte_summary`, `structured_json`, `table_tree`, `text_editor`, `unit_change_view` — and the assets each can draw on are the Console's and Core's own bounded vocabulary. Syntax grammars sit here: a `text_editor` binding names a grammar in its config, and the Console owns what that grammar _is_. The direction of knowledge matters. If the Console held a table from contributed class ids to grammars, it would be deciding a domain question — and it did, until this release. Now the extension says what it wants and the platform says what it has, neither knows the other's vocabulary, and a grammar the build does not have degrades to plain text rather than to a guess. Adding one is additive Console work with no domain-model consequence at all, and v0.3.4 ships no executable frontend extension code.

## Draft's Own Observer Is An Adapter Like Any Other

The filesystem observer is Core, not an extension. It needs no package, no attestation and no grant, and its observation provenance says so: a Core observation record carries a component name and an implementation revision, and it has nowhere to put a producer reference, a trust attestation or a capability grant. That is structural, not a convention — nothing can fabricate one for it.

What it does not get is a private path. Everything above it reaches it through `ResourceSource`, exactly as it reaches a contributed adapter, and it sits in the same registry under the same one-adapter-per-scheme rule. That is not tidiness. A built-in observer with a shortcut is how a port ends up shaped around a single implementation: whatever the filesystem needs quietly becomes reachable without the contract, and the first real adapter discovers the contract was never sufficient. Driving Draft's own observer through the trait is what keeps the trait honest — a capability the filesystem enjoys is one the port offers everybody.

The same rule governs restoration. Draft owns the restore plan, its target locators, its preconditions and its authority; how an opaque anchor becomes state again belongs to the owning adapter and is reached the same way. There is one implementation of each semantic operation, and a CI gate holds it there, because the day two implementations disagree the receipt would still claim they had not.

## Observation Semantics Are Pinned, And ChangePack Only On Purpose

What a project is allowed to see decides what a snapshot even contains. If that could change the instant a package was installed, history would silently acquire additions and removals nobody made — a dependency cache entering the universe looks exactly like somebody adding ten thousand files.

So the semantics in force are **persisted and pinned**. Every observation is taken under the context the project _adopted_, never under whatever happens to be installed at that instant — and asking Draft what context is in force answers with the adopted one, not with what the installed extensions add up to. Those are different questions, and the difference between them is precisely the pending context. Installing, updating, disabling or removing anything whose effective adapter or view-rule semantics differ records a `PendingObservationContext`: a candidate sitting beside the active one, changing nothing. A person previews what adopting it would do, and adoption is explicit, atomic and audited — a new baseline observed under the new semantics, a transition record, and the active pointer, installed under the project lease in that order so a crash can only leave old-with-old or new-with-new.

Work derived under a retired context becomes `ContextSuperseded`. It stays readable: it recorded something true, and still does. What it loses is the right to be mutated or promoted, because there is no longer a shared frame in which to compare it to the project.

One case is deliberately not a transition. A package update whose effective observation semantics are identical — same schemas, same configuration, same coverage partition, same executable identity — changes only _who observed_, never _what is observable_. That is the ordinary upgrade; it produces a new observation-provenance record and nothing else. Forcing a rebaseline for it would train people to click through the very prompt that exists to make them look.

## Acceptance Judges The ChangePack; It Never ChangePacks It

`AcceptanceContext` is assembled from policy alone — protections, verification gates, risk thresholds, reviewability budgets, waiver and approval rules, gap tolerance, recovery-readiness policy. There is no parameter through which a snapshot, change set or candidate could reach it. That is what makes "tightening a policy never supersedes work" structural rather than a rule somebody has to remember: a policy edit invalidates a _readiness answer_, which is exactly what changed.

Each requirement carries its own digest, derived from its own half of the policy. A human decision is recorded against that digest, so re-evaluating after an unrelated policy edit reuses it: strengthening one requirement asks for one new decision, not a fresh round of everything.

Provenance is kept separate again. `AcceptanceContextProvenance` names the exact Core and extension artifacts a context was assembled from, and is _not_ part of the context digest — so two package revisions with identical policy semantics produce identical requirements, keep every prior decision valid, and still let a receipt say which artifact it actually relied on. It is stored under its own digest rather than its context's, because one set of semantics can be assembled from several different sets of artifacts over a project's life and none of those assemblies may overwrite another.

A submission therefore binds three things: the requirements in force, the judgement made against them, and the artifacts those requirements came from. All three are persisted before the receipt that names them, so a receipt can never reference a record that is not there.

Two requirements are not waivable, for two different reasons. A protection exists precisely to be the thing nobody can wave through in a hurry. Review and approval are the human judgements themselves — and a waiver is also a human judgement, so allowing one to stand in for them would mean somebody signing off on not having to sign off. Both the evaluator and the waiver policy the context digest commits to read that rule from one place, so a context can never claim something is waivable that the evaluator would refuse.

Readiness answers in typed states, never in sentences. The blocker list is prose for a person; the machine contract is the verification aggregate — one of `Passed`, `Failed`, `Unavailable`, `NotEvaluated`, `NotApplicable` — alongside observation and derivation completeness, risk assessment, recovery readiness, and the two acceptance digests. Deciding anything by searching a blocker for a word would mean improving the wording could change what a gate does.

## Data Flow

1. A user or agent changes project state.
2. Draft observes it through the resource adapters the active `ObservationContext` names — always including its own filesystem observer, which excludes `.draft/` unconditionally.
3. A checkpoint records a baseline snapshot, and captures a recovery anchor for each resource under the same live fencing that produced the observation.
4. A ChangePack derives the authoritative `ChangeSet` between two snapshots: proved changes, plus a derivation gap wherever coverage could not establish presence or absence.
5. Verification and risk attach evidence and policy inputs. Both report what they could not establish rather than defaulting.
6. A Gate evaluates every required condition over the exact revision, and an immutable Decision authorizes or refuses — against review units that are resource-level by default and finer where a comparison capability derived them.
7. Promotion commits: it preallocates its receipt id, signer binding and Activity event ids, journals its intent, and advances the accepted Baseline through a locked compare-exchange with the ChangePack's lock held, so `Active → Completed` is deterministic finalization rather than a second, separately-failable step.
8. Publication optionally delivers that Baseline to an external provider. It is separately identified, separately journalled, and has no authority over what the project accepts.
9. Every important transition appends a framed, hash-chained Activity record; promotions and publication outcomes issue signed receipts entered in the transparency chain.

## What Draft Reports When It Cannot Know

None of these silently satisfies a gate, and none is a synonym for another:

| State                                    | Means                                                       |
| ---------------------------------------- | ----------------------------------------------------------- |
| `Unavailable`                            | no authorized capability exists to answer                   |
| `NotEvaluated`                           | a capability existed but evaluation did not complete        |
| `NotApplicable`                          | applicability was determined, and nothing applies           |
| `Unassessed`                             | no risk rule is configured or contributed — never `Low`     |
| `PresenceUncertain` / `AbsenceUncertain` | coverage could not prove the resource was there, or was not |
| `Indeterminate`                          | two claims cannot be related; fails closed                  |
| `ObservableButNotRestorable`             | the state is known, but nothing retained can recreate it    |
| `StaleObservation`                       | the resource moved on between observation and use           |

`checks.is_empty()` can never mean `Passed`, and an optional pass never masks a required gap.

## Recovery And Rollback

A `state_digest` proves what was observed. It does not prove Draft can recreate it. That takes a **recovery anchor**: contemporaneous evidence, captured at the target time under the same live fencing as the observation, holding enough material to recreate the _complete_ resource state — not merely its bytes. A stale capture yields no anchor rather than a mismatched one.

Rollback therefore restores target **presence and absence** both, and reports one of three outcomes. `Complete` requires all of: authoritative target knowledge, sufficient retained material, authorized and fenced restoration, and a complete post-restore observation whose every locator and state digest equals the target's. A successful mutation is not completion.

Where the target-time observation had a gap, that gap is permanent for that receipt. A domain nobody observed then has no resources and therefore no anchors, and observing it today reveals today's state — not the target's.

## A Read Model Is Never An Authorization

A view is what somebody saw. Between reading it and acting on it, the project can move — and nothing about that looks like an error at the time. It looks like the action worked.

So every state-sensitive action carries the authoritative watermark its offer was computed against:

```
ReadModelWatermark {
    project_control_generation,
    activity_tail_hash,
    store_generations: { ChangePack, ProviderBinding, PublicationControl,
                         Task, Evidence, Gate, Decision },
}
```

and the watermark is re-read from the stores and compared immediately before the mutation. A caller acting on a stale view is refused with `StaleAction`, naming what moved, and `read_model_stale_rejections` counts it.

**Comparing the descriptor with the request is not this check.** An invocation that agrees perfectly with the descriptor it was issued proves only that the client echoed what it was given; it says nothing about whether the project moved in between, and that window is exactly where a stale action lands.

Invalidation is a map rather than a counter, and is specific about what it does _not_ affect. One global counter would be correct and useless: every write would invalidate every view, a busy project would refuse constantly, and people would learn to retry blindly until something stuck — which is worse than no check, because it trains the habit that defeats it. A `ProviderBinding` reprofile changes where new work would be sent; it cannot change what an accepted Baseline was composed from, and invalidating that projection would be a lie about which facts can move.

A projection that depends on a store its watermark never recorded is stale, not fresh. An unanswerable question is not a passing answer.

## Operational Telemetry Is Never Evidence

Counters say how often Draft took a path. They never say what a project decided. No counter value participates in a canonical digest or a receipt payload identity, none is durable, and none substitutes for an Activity event — the ledger is the record of what happened, and a metric is a description of a running process. A restart resets every counter, which is right for a number about a process rather than a project. Trace, span and request ids stay out of canonical identity entirely, and nothing observable through telemetry may carry a credential secret, token, private key, raw secret handle or sensitive extension environment value.

The counter vocabulary is frozen: an operator's dashboards are written against the names, so a counter is retired rather than repurposed, and no name may imply a canonical Publication fact type.

**An unemitted counter reads as unknown, not zero.** Returning `0` for a counter nothing emits would make "we never saw this" and "nothing can see this" indistinguishable — the same mistake as inferring complete observation from an empty Resource set. So the registry returns `Option<u64>` and `None` means nothing emits it.

Every counter in the frozen vocabulary is now attached to a production emission boundary, so that `None` is unreachable in a released build. `scripts/check-telemetry-completeness.sh` enforces all three directions: a counter claimed as emitted with no call site, a call site for a counter the registry does not declare, and any frozen counter missing from the emitted set at all. A call site inside a `#[cfg(test)]` module does not count — a test could otherwise satisfy the gate while the boundary it describes emitted nothing in a real run — and the frozen names are checked against the vocabulary exactly, because a rename is a breaking operational change.

Where a counter is declared rather than incremented in place — the per-Store CAS conflict counters, which the guarded compare-exchange increments from inside the critical section that lost — the declaration _is_ the emission site, and the gate recognises it as one. That is not a shortcut: the Store is the only place that knows which counter its own conflicts belong to.

Counters are process-local, so the daemon is where they are worth reading: `service.telemetry` returns the snapshot, and a CLI process reports only the command that just ran.

## Store Authority

Canonical JSON records and the framed Activity Ledger are the durable source of truth. SQLite indexes are rebuildable caches. Object files are content-addressed by hash.

## Safety Boundary

`.draft/` is private Draft metadata. It is not project state and never a change candidate, through any contributed selector or view rule. Any implementation that introduces `.draft/` into status, snapshots, ChangePacks, promotion, recovery, or external command execution is a release blocker.

Contributed commands run outside the project entirely, in a per-operation runtime root under the system temporary directory. That boundary is about **authority, budget, provenance and evidence** — argv-only spawn with no shell, a cleared environment, a timeout, bounded and redacted output, and a recorded executable identity. It is deliberately **not** an OS sandbox: there is no filesystem jail, no network isolation and no namespace confinement, and nothing in Draft claims otherwise. `OperationBoundaryViolation` means Draft detected an unexpected effect inside the state it monitors; it does not prove the absence of effects elsewhere.

## Local Services

Draft services are optional local helpers. The CLI invokes `draft-core` directly and remains fully usable without them.

### `draftd`

`draftd` is the local control-plane process. It accepts IPC requests, validates their shape, dispatches core operations, coordinates local sessions, and returns structured responses. It powers live or background flows for clients that need them without changing Draft's authority model.

### Durable Jobs

The service store records durable local jobs for scan, evidence, assessment, compose, promotion, publication, recovery, and index-rebuild requests. Jobs can be queued, running, completed, failed, or cancelled. Execution delegates to `draft-core` and stores the result or error.

### IPC

The IPC crate provides versioned, JSON-encoded local request/response transport. Unix platforms use a local socket path; non-Unix platforms use a loopback fallback. Tests cover status dispatch, malformed requests, unknown methods, workspace errors, lifecycle helpers, durable job operations, and fallback transport behavior.

### Watcher

The watcher debounces filesystem events and filters Draft write-back paths. It can trigger refreshes and background work, but core scanning remains the authoritative view of the workspace.

### Locks, Sessions, And Registry

The locks service supplies cross-platform local file guards for operations such as event append. Sessions are lightweight in-memory records for connected clients and request ownership. The service store records daemon metadata so clients can discover local service status.

### Draft Console Service

`draft console web` starts the authenticated browser gateway on loopback. `draft console tui` starts the terminal frontend. Both require `draftd`; all domain validation, transitions, persistence, canonical read models, revisions, action availability, and side effects remain in `draftd`, services, and `draft-core`. The Rust-facing Console protocol negotiates only after the normal Draft IPC transport handshake and reuses its endpoint, framing, peer model, and lifecycle.

The existing `~/.draft/adapters/agui/` namespace is reserved for intentional external AG-UI adapter configuration. It is unrelated to the browser Console package and ownership boundary.

Extension packages can be installed, inspected, enabled, disabled, and uninstalled with `draft extension`. Their entrypoints are not executable through the Draft CLI.

### Reserved Boundary

The `sync` crate is intentionally no-network in v0.3.4. It reserves a named boundary for later design work without changing current local-first behavior.
