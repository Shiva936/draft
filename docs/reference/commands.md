# Command Reference

The Draft CLI is the primary interface. Core workflows are local-first and work without a daemon.

Human-readable output is the default CLI contract. Machine-readable output is available only where command help declares `--json` or `--raw`; v0.3.4 does not add those flags to every command.

The v0.3.4 surface centers on `init`, `status`, `inbox`, `doctor`, `maintenance`, `config`, `daemon`, `project`, `task`, `pack`, `resource`, `baseline`, `activity`, `authority`, `recover`, `extension`, `console`, `update` and `uninstall`. Each stage of the Change Graph — evidence, assessment, gate, decision, promotion, publication — is a separate command because each is a separate act.

## Workspace

### `draft init [-b <base-change-name>]`

Initializes `.draft/`, creates the project event stores, writes default config files, creates the base ChangePack, and selects it. The default base ChangePack name is `base`.

First-run output includes next actions: `draft task wizard`, `draft task list`, and `draft console tui`. If no command candidates are configured, Draft says so and points to `[candidates.<name>]` in `.draft/config.toml`; human/manual tasks still work without candidate setup.

### `draft status [-p <cpk-id>] [-c repo|tasks|candidates|changes|hooks] [--full]`

Shows workspace or ChangePack status. Component filters return focused status for repository metadata, task health, resolved candidates, workspace changes, or hooks. `--full` includes backing records; the default stays compact. `.draft/` is always hard-excluded.

### `draft pack checkpoint <message>`

Creates a checkpoint with a `chk_` ID and a receipt.

## Doctor And Recovery

### `draft doctor [--global] [--json]`

Validates the project and global Draft stores, including metadata, event and receipt integrity, signing-key state, indexes, and recoverable journal operations. `--global` limits the report to the user-scoped store.

### `draft doctor sync [--fix] [--json]`

Inspects the global project registry. `--fix` removes or repairs stale registry entries; without it, the command is read-only.

### `draft doctor storage [--json]`

Reports on object storage without changing it. `doctor` is read-only throughout; everything that rewrites storage lives under `draft maintenance`.

### `draft doctor activity [--replay] [--json]`

Verifies the Activity chain. `--replay` additionally folds it in memory and reports what it contains, by event kind. Both are read-only: repairing a derived index is `draft maintenance index-rebuild`, and Doctor never rewrites history.

### `draft doctor index [--refresh] [--global] [--json]`

Reports whether project or global derived indexes are fresh, stale, missing, or failed. `--refresh` rebuilds the selected scope before reporting it.

## Config, Hooks, And Ignore Rules

### `draft config get <key> [--global]`

### `draft config set <key> <value> [--global]`

### `draft config unset <key> [--global]`

Reads and writes config. Without `--global`, reads use project-over-global precedence and writes target the project. With `--global`, reads and writes target only the user-level `~/.draft/config.toml`.

The only mutable user profile is `user.name` and optional `user.email` through these commands. Both reject empty or whitespace-only values. Use `draft config unset user.email` for absence. Profile metadata cannot change the stable security actor, keys, signatures, authorization, trust, attribution, ownership, receipts, hashes, or digests.

### `draft config hook set <key> <value> [--global]`

### `draft config hook unset <key> [--global]`

### `draft config hook run <hook-name>`

Manages project hook configuration; inspect a configured value with `draft config get hooks.<key>`. Draft has no native commit, push, pull, sync, PR, MR, publish, host-specific, or remote commands.

### `draft config ignore add|remove|list`

Manages `.draft/.ignore`. `.draft/` remains hard-excluded even if ignore rules are changed.

## Activity

### `draft activity list [--page <page>] [--limit <entries>] [--raw]`

### `draft activity show <event-id>`

### `draft activity verify`

Renders a clean human-readable timeline derived from `.draft/events/events.log`, the sole authoritative Activity file. `--raw` prints each logical record as JSON for audit, debugging, replay, and tooling. Use `draft doctor` or `draft doctor receipts --all` to verify the Activity chain, the receipts, and the transparency chain. There is no `draft log`, and `draft activity list` accepts only long `--page` and `--limit` pagination flags.

## Packs

### `draft pack list [--json]`

Every ChangePack and the revisions sealed against it. ChangePack IDs use `cpk_`.

### `draft pack show <cpk-id> [--json]`

One ChangePack: its lifecycle, what it is for, what it may touch, and every revision sealed against it.

### `draft pack intent show <cpk-id> [--json]`

What the ChangePack is for, in the author's words. Opaque to Draft.

### `draft pack intent set <cpk-id> <intent> [--json]`

### `draft pack intent amend <cpk-id> <intent> [--json]`

Restates what a ChangePack is for. Both spellings are the same act, kept apart only because "state it for the first time" and "change what it says" read differently to the person doing it.

An intent lives in the ChangePack's definition, and a definition is an immutable fact — so this mints a new one and moves the ChangePack's pointer at it rather than editing what a reviewer may already have read, recording `ChangePackDefinitionAmended`. The declared scope is carried across unchanged: amending an intent is not a way to widen what the work may touch. Any scope already resolved against the old definition is left stale by construction, so a ChangePack amended after resolution is re-resolved rather than silently sealed under a boundary nobody approved.

### `draft pack select <cpk-id> [--json]`

Chooses the ChangePack subsequent commands default to. A convenience, never an authority: every command that acts still names the exact revision it acts on, and selecting one cannot widen what any of them may do.

### `draft pack inspect <cpk-id> [--json]`

Everything recorded about a ChangePack and its newest revision at once — every judgement, the explanation of its work, and what it currently interferes with. `show` answers what the ChangePack _is_; this answers what has _happened_ to it, and is correspondingly more expensive.

### `draft pack depends <cpk-id> [--json]`

The accepted history a ChangePack was worked from: the Baseline its newest revision was sealed against, that Baseline's lineage, and the promotion that produced each ancestor.

Lineage, not proximity. Two ChangePacks touching neighbouring Resources depend on nothing of each other; `draft pack conflicts` is the question that asks about them.

### `draft pack conflicts <cpk-id> [--json]`

Every other ChangePack whose newest sealed revision interferes with this one's, and why. A ChangePack with no sealed revision has touched nothing yet and is absent: reporting it as a conflict would make every open ChangePack look like an obstacle.

### `draft pack compose <cpk-id> <cpk-id> [...] [--json]`

### `draft pack disperse <cpk-id> <cpk-id> [...] [--json]`

Asks whether several sealed revisions hold together against one Baseline. A composition is `verified` only when every pair is independent **and** every member was sealed from the same Baseline; anything else is `failed`, with each pair naming what stands in the way.

`disperse` is the inverse, and is not a mutation: for each member it reports whether that member can be advanced on its own, and — when it cannot — the exact relations holding it. The answer says what to resolve rather than merely refusing.

There is deliberately no dependency ordering. What a revision was built on is its base Baseline, which `draft pack depends` reports exactly; ordering by a declared dependency list would be a second, weaker answer to a question accepted history already answers.

### `draft pack impact <rpk-id> [--json]`

What a revision reaches: every element an authorized extractor found inside the Resources it touched, and the Resources reachable from those elements through a contributed relation.

Nothing is inferred. An element exists because an extractor said so and a relation exists because one said so; directory layout, dependency edges, graph proximity and name similarity produce no elements at all. Resources nothing installed can extract from are reported as `unextractable` — a real answer, since without it "no elements" would mean both "nothing is in there" and "nothing knows how to look".

### `draft pack coverage <rpk-id> [--json]`

What the evidence about a revision actually speaks for, Resource by Resource: `direct`, `indirect`, or `uncovered`.

Deliberately hard to satisfy. **Direct** coverage means the evidence read an observation of that exact Resource — nothing weaker. **Indirect** means somebody asserted it, through a declared coverage relationship or a producer attestation. Same directory, imported by, adjacent in the graph, reachable from something tested and named similarly are each rejected: every one of them would produce a confident `covered` for a Resource nothing has ever verified.

Indirect sources report their own availability. "No declared relationship asserts this" and "Draft has nowhere to record such an assertion" are different facts, and v1 records neither kind of assertion — so the answer says so rather than reporting their absence as though the project had been checked.

### `draft pack representation list [--json]`

### `draft pack representation show <rpk-id> [--json]`

The derived explanation of what a revision did, recorded when the revision was sealed and bound to that exact revision.

Each touched Resource gets an entry naming the strategy that explains it — a contributed presentation where one claims the Resource, and otherwise the neutral rendering that always exists and no extension contributes. The neutral rendering says exactly what Core can justify: which Resource changed, between which two authoritative state digests, and that the claim covers the whole Resource because Draft cannot say where inside it the work landed. It does not diff content or interpret a domain, because doing so would make Core the semantic authority for every kind of Resource.

Representations are what let `compare`, `compose` and `conflicts` give a finer answer than whole-Resource overlap.

### `draft pack receipts <cpk-id> [--json]`

The receipts issued for this ChangePack's promotions.

### `draft pack scope <cpk-id> [--json]`

What the ChangePack may touch — both the declaration and the resolution, because a reader given only the resolved set cannot tell whether a declaration was narrowed. Resolution may narrow a declaration but never widen it, and it is resolved once and verified again at seal, so a definition amended afterwards leaves the resolution stale rather than silently widening what the work reaches.

### `draft pack revision seal <cpk-id> [--json]`

Seals the workspace's current state as a revision of a ChangePack. The state is observed, not asserted. Sealing the same state twice is the same revision, so a re-run after a dropped connection is not a second thing to review.

Sealing also records the revision's representation, derived from the same observations the revision was sealed over. Deriving it later would explain a workspace that has since moved.

### `draft pack revision list <cpk-id> [--json]`

### `draft pack revision show <cpk-id> <rpk-id> [--json]`

The revisions sealed against a ChangePack, newest first, and one of them in full: the exact definition and scope it was sealed against, the Baseline it was worked from, and the resources it touched.

### `draft pack abandon <cpk-id> [--json]`

### `draft pack reopen <cpk-id> [--json]`

There is deliberately no delete. Abandoning is a statement about the _future_ — no more revisions, no more decisions — and says nothing about the past: every definition, revision, decision, receipt and event stays exactly where it was, and `draft pack list` still shows it. The record of work that was done and then decided against is frequently the part worth keeping. `draft pack reopen` resumes an abandoned ChangePack. Both transitions appear in Activity as `ChangePackAbandoned` and `ChangePackReopened`.

`draft pack reopen` answers a different question: it takes an abandoned ChangePack and makes it active again. A ChangePack whose work is already in an accepted Baseline cannot be reopened this way — create a successor ChangePack instead. Nothing recorded about the ChangePack is invalidated: Evidence, Assessments, Gates and Decisions each bind one exact revision and simply keep describing the revision they were made about.

### `draft pack compare <cpk-id> <cpk-id> [--json]`

Reports how two ChangePacks interfere over the resources they both touch, resource by resource: `independent`, `conflicting`, or `indeterminate`.

Resources only one side touches are absent from the result. Silence is the answer for them, and listing them as independent would bury the ones that actually interfere.

The answer is computed from the two change sets as they are now rather than read from a stored verdict, because a ChangePack that moves invalidates a composability claim made about it earlier — and a cached one would keep asserting it. Where neither side has derived a finer representation of what it changed, two ChangePacks that both touch a resource cannot be shown separable, so they are reported as interfering rather than assumed composable.

## Candidates And Tasks

### `draft pack candidate list|show|remove`

### `draft pack candidate add <name> [--kind command|chat|manual] -- <template>`

### `draft pack candidate update <name> [--kind command|chat|manual] -- <template>`

Manages host-agnostic candidate execution profiles. Missing candidates referenced by task spawn are auto-registered.

### `draft task spawn "<name>" [-p <cpk-id>] [-c <candidate-name> ...] [--cron <expr>] -- <instruction>`

### `draft task create <name> --goal <goal> [--template <id>] [--candidate-preset <id>]`

### `draft task wizard`

### `draft task list`

### `draft task show <task> [--full]`

### `draft task update <task> [--status <state>] [--priority <priority>] [--due <RFC3339>|--clear-due] [--assignee <id> --assignee-kind actor|candidate|--clear-assignee]`

### `draft task next-action <task> add <label>` / `complete <action-id> [--reopen]`

### `draft task drop <task> [--hard]`

### `draft task templates`

### `draft task export <task> [--output <path>]` / `draft task import <path>`

Lists the task templates available — Draft ships none, because a template is domain vocabulary and arrives from an installed contribution — and moves a task definition between projects.

Creates, inspects, spawns, and retires task definitions. `draft task create` validates template ids, candidate presets, success criteria, zones, protected paths, and schema round-trips before writing. Template ids are namespaced and contributed: Draft ships none, so `--template` resolves against installed `task_template` contributions and names what is available when it cannot. A template narrows what a task is about; a step scoped by anything a task zone cannot express is refused rather than silently unscoped. `draft task wizard` uses deterministic prompts for task name, template, goal, allowed/forbidden zones, success checks, risk, plan-first mode, and candidate preset; it prints a preview and only writes after confirmation. `task spawn` records task/candidate/ChangePack provenance and supports stored tasks, inline instructions, candidate presets, and execution lifecycle flags (`--resume`, `--cancel`, `--retry`). `task drop` clears execution/runtime state while keeping the definition; `--hard` removes the task definition and journals the operation.

### `draft inbox [--json]`

Lists items requiring attention: ChangePacks needing review, failed or resumable executions, owner review gaps, waiver renewals, doctor recovery warnings, and pending resource edits. Every item includes a next safe action.

## The Change Graph

The chain that decides what this project accepts, and what leaves it. Each stage is a separate command because each is a separate act:

```
evidence → assessment → gate → decision → promote → publish
```

A gate says whether conditions are satisfied. A Decision authorizes. A Promotion is the only operation that changes the accepted Baseline. A Publication delivers that Baseline somewhere else and has no authority over it — a failed publication leaves the Baseline exactly as accepted as it was.

### `draft baseline list [--json]`

Every Baseline this project has accepted, newest first, with the origin of each and which one is currently accepted.

### `draft baseline show [--json]`

Shows the Baseline this project accepts: its three roots, its lineage, what established each Resource's state, and whether a publication of it could be routed right now. The Baseline is the project's authoritative state; there is no other. `draft baseline` with no subcommand does the same thing.

### `draft baseline current|state-root|evidence-root|coverage|lineage|composition [--json]`

One projection each, so a reader can ask one question at a time. `state-root` is what material state is accepted; `evidence-root` is what exact provenance establishes it; `coverage` is what justifies absence, per provider and domain, distinguishing _not observed (no attempt)_ from _not observed (attempt failed)_. An empty Resource set never proves complete observation.

`composition` renders `accepted_provider_provenance` — immutable, and never a route — beside whether delivery could be routed right now. The first never changes when a binding is reprofiled or unbound; the second moves whenever the binding does.

### `draft baseline receipts [<rcp-id>] [--json]`

The receipts this project has issued, or one of them in full. Receipt IDs use `rcp_`. Reading a receipt is not verifying it: this prints what a receipt attests, who signed it and that a signature is present — never that it is valid, because that is a different question, and it is answered by `draft doctor receipts`.

### `draft baseline publications [--json]`

Every Publication and where each one is.

### `draft pack new <intent> --scope <res-id> [<res-id> ...] [--json]`

Opens a ChangePack: what it is for, and exactly what it may touch. The declared scope is resolved against the accepted Baseline, which may narrow it but never widen it. Re-running with the same intent converges on the ChangePack it opened.

### `draft pack evidence run <rpk-id> [--json]`

Runs every check that applies to the changed resources, aggregates the results into one of five states, derives the impact index from contributed extractors, and records immutable Evidence bound to the exact revision it was gathered about.

Which checks apply is decided by the project's own `verify.toml` and by contributed checks whose predicates match the changed resources. A check that cannot run — because no authorized capability exists — is still selected and still reported, so a missing capability can never quietly shrink the required set.

The aggregate is `passed`, `failed`, `unavailable`, `not_evaluated` or `not_applicable`. Only `passed` satisfies a gate condition on its own; every other state needs an explicit waiver naming it. With nothing installed the result is `unavailable` — true, emphatically not a pass, and distinct from `not_applicable`, which would mean a check existed and nothing in this change was in its scope.

Runs the project's configured checks against a sealed revision and records the result. Refuses if the workspace no longer holds the state that revision sealed: checks that ran over different content say nothing about the work reviewed.

The outcome is one of five — `passed`, `failed`, `unavailable`, `not_applicable`, `not_evaluated` — because "no checks ran" and "checks ran and passed" are different facts, and only one of them is a reason to proceed.

### `draft pack assess <rpk-id> --risk low|medium|high|critical [--rationale <text>] [--json]`

Records a risk judgement over a revision's evidence. There is deliberately no way to record "unassessed": that is what Draft concludes when nobody has looked, not something to assert.

### `draft pack gates evaluate <rpk-id> [--waiver <wvr-id> ...] [--json]`

Evaluates the project's gate over a revision. Every required condition is reported, satisfied or not. A waived condition is recorded as satisfied _and_ names the waiver that excused it — "somebody allowed this" never looks like "this passed".

### `draft pack gates list <cpk-id> <rpk-id> [--json]`

Everything decided about a revision — evidence, assessments, gates, decisions, any promotion — and which actions are currently legal, with the reason for each that is not.

### `draft pack gates waive <rpk-id> <condition> --reason <text> [--days <n>] [--json]`

Excuses one gate condition on one exact revision. Bound to the revision, not the ChangePack: an exception accepted for the work as it stood is not an exception for whatever it becomes.

### `draft pack review <rpk-id> [--comment <text> ...] [--json]`

Records that you examined a revision. A review is the act of looking; a Decision is the conclusion. They come apart in both directions — a reviewer can read a revision and conclude nothing yet, and a decision with no recorded review behind it is precisely what an audit wants to notice.

Re-recording the same review merges new comments into the existing record rather than accumulating one entry per invocation.

### `draft pack decide <rpk-id> --approve [--gate <gate-id>] [--json]`

Records an immutable approval, which authorizes a promotion. It does not perform one. An approval requires a satisfied gate over the same revision.

### `draft pack decide <rpk-id> --reject --reason <text> [--json]`

Records an immutable rejection. Needs no gate: refusing work is legitimate whatever the checks say.

### `draft promote <cpk-id> <rpk-id> [--decision <dec-id>] [--gate <gate-id>] [--expected-baseline <digest>] [--json]`

Promotes an authorized revision, advancing the accepted Baseline. The only operation that changes what this project accepts.

The decision and gate default to those on record for the revision. `--expected-baseline` states the Baseline you believe is accepted; it is read from the project when omitted, which is right for a command invoked now. Pass it explicitly when acting on a view taken earlier: a promotion decided against state that has since moved is refused rather than silently rebased.

Retrying the same promotion converges on the one already made rather than producing a second Baseline.

### `draft promotion <pro-id> [--json]`

What a promotion did and how far it got: `pending`, `blocked`, `running`, `recovering`, `completed` or `failed`, with the Baseline it accepted.

### `draft authority grant [--capability <id>] [--to <act-id>] [--json]`

Grants one capability over this project, defaulting to `draft.publish/v1`. Publishing is its own capability: being permitted to accept work into a Baseline is not being permitted to announce it to the outside world, and collapsing the two would make every approver an unwitting publisher.

The capability is named rather than assumed, so a reader of the record never has to guess what was permitted. A reserved `draft.*` capability this build does not implement is refused rather than recorded as a permission nothing will ever check.

Granted over the project rather than over an individual Publication, and recorded as a fact each attempt cites. Until it exists, `draft baseline publish run` refuses with `CAPABILITY_NOT_AUTHORIZED` before anything external happens. The grant id is derived from the capability, the project and the grantee, so re-running converges rather than minting a second grant saying the same thing.

### `draft authority list [--json]`

### `draft authority show <auth-id> [--json]`

Every grant this project has issued, with its current standing, or one of them. Revoked grants are listed too: a record of authority that drops what was withdrawn cannot answer the question an audit actually asks.

Issued and in force are separate facts. A grant is an immutable record that somebody permitted something; whether it currently confers anything is the project's security state's answer, read under the control lock.

### `draft authority revoke <auth-id> --reason <text> [--json]`

Withdraws a grant. Records an immutable `AuthorityRevocation` naming the grant by exact reference, then moves that reference out of the project's active security state under the control lock, recording `AuthorityRevoked`.

Nothing is deleted. A receipt issued while the grant was live stays valid history — a later revocation blocks **new** operations and never rewrites old ones. The reason is required: "revoked for cause" and "revoked because the project finished" lead to different follow-up, and neither is recoverable from the bare fact that a revocation exists.

### `draft authority executor grant|list|revoke`

The same machinery over `draft.change.operate/v1`. An agent permitted to carry out operations on this project has not thereby been permitted to announce anything outside it.

### `draft baseline publish run [--baseline <digest>] [--purpose <id>] [--attempt <id>] [--json]`

Delivers a promoted Baseline outside Draft. Optional and repeatable. Refuses a Baseline the project never promoted — the initial Baseline is exactly that.

Requires publish authority (see `draft authority grant`). Before the delivery, Draft records what the effect happens under: the grant it cites, the project's control generation, its policy and security state, the trust registry revisions, the binding generation, and a fenced lease. Those are frozen into an immutable attempt, so what authorized the effect can be read back afterwards rather than inferred.

`--attempt` identifies this attempt. Re-running with the same value converges on what that attempt concluded instead of delivering again; omit it for a new send. What a repeated send is allowed to do follows from the target's delivery semantics, which the binding declares — it is not something a caller chooses.

### `draft baseline publish list [--json]`

Every Publication and where each one is, per target, folding attempts into primary outcomes and active resolutions. Status wording distinguishes **dispatch committed** from **provider result**: a dispatch that was authorized and durably committed but whose outcome is not yet known reads as _dispatch committed, result pending reconciliation_, never as "attempted and succeeded/failed".

### `draft baseline publish authorize-retry --rationale <text> [--purpose <id>] [--json]`

Authorizes exactly one further attempt at a delivery Draft could not establish.

A delivery that ended indeterminately against a target whose semantics cannot rule out duplication is stuck on purpose: re-sending might duplicate a real-world effect, and nothing Draft can read locally says whether it would. This is the decision that unsticks it, recorded with who made it and why. The authorization is one-shot and bound to that exact Publication, and consuming it is atomic with allocating the attempt — so two processes cannot both spend it.

A historical authorization is not a current permission: a later dispatch still performs its normal current-security validation and may refuse.

### `draft baseline publish retry --retry-authorization <digest> [--purpose <id>] [--attempt <id>] [--json]`

Attempts the same Publication again, on the same route and idempotency key, as a new attempt.

Requires an authorization, because the only reason to reach for this is a delivery whose effect Draft could not establish — and re-sending one of those may duplicate a real-world effect. Delivering a Baseline again _on purpose_ is `draft baseline republish`, which is a different Publication with its own identity and history.

### `draft baseline publish resolve [--outcome <digest>] --mark-succeeded <ref> | --mark-failed <reason> --rationale <text> [--purpose <id>] [--json]`

Records what was later established about an uncertain delivery.

The primary outcome is never rewritten. "We did not know, then we learned" is a different history from "we knew all along", and only one of them explains why a retry authorization was issued in between — so this writes a Resolution beside the outcome under **current** resolution authority. The old dispatch grant never carries forward to authorize it.

`--outcome` names the exact outcome being resolved; omitted, it resolves the Publication's most recent conclusion. When supplied it must match, because attaching an interpretation to the wrong fact is exactly what the exactness rule exists to prevent.

### `draft baseline republish [<baseline>] --republish-intent <id> [--purpose <id>] [--attempt <id>] [--json]`

Delivers an already-published Baseline again, under a stated intent.

A different Publication, not a retry of the old one: the intent is part of the request key, so the second delivery has its own identity, its own attempts and its own history.

### `draft baseline publish withdraw --attempt <id> --reason <text> [--json]`

Withdraws an attempt that stalled before it was sent.

An attempt interrupted between its allocation and the moment it would have been delivered blocks its Publication, because the barrier cannot prove it caused no effect. This proves it — the journal never reached the dispatch boundary — and frees the Publication. An attempt that was already dispatched is refused, because withdrawing it would assert something Draft cannot know.

### `draft recover plan <chk-id|cpk-id|evt-id>`

### `draft recover run <chk-id|cpk-id|evt-id>`

### `draft recover dry-run <chk-id|cpk-id|evt-id>`

Restores a past state, inferring the target type from the ID prefix. `plan` resolves the target and reports what would be restored and what would be **removed**, without mutating anything; `run` performs it.

An `evt_` reference names the Activity event that recorded a checkpoint. The Activity Ledger is hash-chained and verified, so the event is the durable fact; no signed receipt is involved, because v1 receipts attest Promotions and Publications rather than local actions.

Removals are surfaced separately and by name, because recovery deletes. Recovery always protects `.draft/`.

## Installation

These manage the Draft installation itself, never a project. They run anywhere, need no project, and never read or write a `.draft/` directory.

### `draft update [--check] [--version <semver>] [--channel stable|prerelease] [--allow-downgrade] [--json]`

Updates an official installation to the newest release eligible on its channel (the highest SemVer, not the most recently published). Every artifact is verified against the signed release manifest before it replaces anything; both binaries are replaced as one recoverable transaction, validated by running them, and a running daemon is stopped and restarted. An interrupted update is completed or rolled back on the next run.

`--check` reports and changes nothing. `--version` installs exactly that release and never changes the channel; installing an older one needs `--allow-downgrade`. `--channel` switches the track; when the newest release on the new track is already installed, only the recorded channel changes. `--version` with `--channel`, and `--allow-downgrade` without `--version` or with `--check`, are rejected.

Package-manager, source and unrecognized installations are reported, not modified.

### `draft uninstall [--dry-run] [--purge] [--yes] [--json]`

Removes the executables, their PATH exposure (the Unix symlinks, or on Windows only the User PATH entry the installer added) and the installation's own metadata. `--dry-run` prints exactly the plan the real command runs. The global user store is kept unless `--purge` is given, and `--purge` deletes it only after proving it is a Draft-managed store; `--yes` skips the confirmation prompt and nothing else.

`draft uninstall` does not remove Draft metadata from your projects. Your `.draft/` directories are never scanned and never deleted.

## Providers

### `draft project provider list|show [--json]`

Every provider binding this project has, withdrawn ones included, with the immutable definition and profile it currently points at.

Withdrawn bindings are listed because `unbind` withdraws a binding from new work and deletes nothing. A listing that hid them would make a withdrawal look like a deletion.

### `draft project provider bind <name> --semantics <file> --definition <file> --profile <file> [--json]`

Attaches this project to a provider. Three facts, and only one of them ever moves:

```
ProviderSemanticDefinition   what its namespace and endpoints mean       IMMUTABLE
ProviderOperationalProfile   how it is driven, and what it can do        IMMUTABLE
ProviderBinding              which of those this project points at       revisioned
```

The two immutable facts are supplied as canonical JSON documents read from files, each deserialized _exactly_ into the frozen structure it names. The command line adds no vocabulary of its own, and an unknown field is refused rather than ignored — a definition written against a different idea of the format fails loudly instead of binding a provider to semantics nobody stated.

`--semantics` is the `ResourceStateSemanticsContract` the definition's observations are read under. It is registered first and the definition validated against it: one identifier means exactly one contract forever, so a changed contract under an accepted identifier is refused as a semantic identity conflict rather than adopted. A vendor changing semantics mints a new namespaced identifier.

`<name>` is what makes two attachments of the same kind possible; the binding id is derived from the project, the kind and the name, so re-running converges rather than creating a second attachment meaning the same thing.

### `draft project provider redefine <pbd-id> --semantics <file> --definition <file> [--json]`

### `draft project provider profile <pbd-id> --profile <file> [--json]`

Points a binding at a different immutable definition or profile. History is untouched: every Baseline keeps naming the definition its observations were actually made under, and a route planned against the old one becomes **stale** — refused and re-planned rather than silently re-pointed at the new one.

### `draft project provider unbind <pbd-id> [--json]`

### `draft project provider rebind <pbd-id> [--json]`

Withdraws a binding from new work, and reactivates it. `unbind` deletes nothing: historical verification, reads, GC reachability and explicit recovery all keep working; new observations, routing, delivery and materialization are refused until it is rebound.

Every binding mutation is a journalled, audited transaction — `ProviderBindingAdded`, `ProviderBindingRetargeted`, `ProviderBindingUnbound`, `ProviderBindingRebound` — so a crash between the record moving and the Activity append is decidable rather than a guess.

## Console And Extensions

### `draft console web [--project <workspace-id|path>] [--no-preselect] [--port <n>] [--no-open]`

Starts or reuses `draftd`, then serves Draft Console on an explicit loopback socket (default `127.0.0.1:4317`). `--no-open` leaves the one-time bootstrap URL in the terminal.

### `draft console tui [--project <workspace-id|path>] [--no-preselect]`

Starts the terminal frontend. Bare `draft console` is an error because a mode is required. Both modes work outside a project, select only already-registered workspaces, and never register implicitly. `--project` resolves an exact workspace id or canonical path and conflicts with `--no-preselect`; otherwise a registered workspace containing the current directory is preselected when safe.

### `draft daemon start|stop|restart|status [--json]`

Controls the long-lived local `draftd`. Core CLI workflows remain daemonless-capable; Console requires the daemon and reports a reconnectable offline state if IPC is unavailable.

### `draft project list|register|init|relocate|unregister|adopt-copy`

Manages the canonical global project registry. Relocation verifies the same immutable workspace id at the destination. `adopt-copy` creates an independent identity and preserves the copied Draft store in an adoption backup; it never merges histories.

### `draft project control [--json]`

Shows what this project currently accepts: the accepted Baseline, the policy and security state in force, and whether the project is open to new work. One record answers all of it, so a reader is never assembling that answer from four stores that may disagree.

### `draft project close [--json]`

Closes the project to new work. A lifecycle transition, never a deletion: the history stays readable and verifiable, and every receipt the project issued keeps meaning what it meant. Removing Draft's metadata is `draft maintenance remove-project`, which is a different act with a different consequence.

Closing an already-closed project is refused rather than treated as a no-op — "closed" is a fact somebody recorded once, and a second recording would claim it happened twice. The transition appears in Activity as `ProjectClosed`.

### `draft extension source add|list|show|remove|delete|enable|disable|trust|refresh`

Configures local-directory or HTTPS catalogs without implicitly trusting them. `--local` and `--https` state which form a location names, so a mistyped scheme is refused rather than silently read as a directory. `source trust <id> --root <file> --fingerprint sha256:...` is the explicit out-of-band trust bootstrap; `--root`/`--root-sha256` on `source add` perform the same decision in one step, and `--reset` is an audited recovery action. Refresh requires a complete, unexpired signed root → timestamp → snapshot → targets chain, enforces signature thresholds and durable version floors, and rejects identity changes, rollback, replay, mix-and-match metadata, and delegation escape. `refresh` with no id refreshes every enabled source.

`disable` stops discovery, refresh and installation from a source while leaving its configuration, trust and installed packages untouched; `enable` resumes it. `remove` and its alias `delete` drop the configuration entirely and still never uninstall anything: installed packages, their provenance and the audit history are retained so an install stays explainable after its source is gone. A source built into the Draft build is disabled rather than removed, because its trust anchor is supplied by the binary.

### `draft extension search [query] [--source <key>] [--capability <capability>] [--page <n>] [--limit <n>] [--refresh] [--json]`

Searches cached signed targets by id, name, description, keywords and contributed capabilities. Discovery metadata rides inside the signed targets role, so a result cannot be steered by unsigned text. Search reads the verified cache and works offline; `--refresh` is the only form that contacts a source. Expired cache remains inspectable with an explicit freshness state but cannot authorize installation or update.

Search is deliberately forgiving. Resolution is not: see `install` below.

### `draft extension install <path>` / `draft extension install <id> [--source <source-id>] [--version <version>] [--grant <permission>]`

Installs either an explicitly selected local directory or a digest-authorized catalog archive. An argument naming an existing directory is a local package; anything else is resolved as an exact canonical extension id. Draft never installs a fuzzy match, and when more than one eligible source publishes the same id it refuses and lists the candidates rather than choosing — pass `--source` to say which one you mean.

Every package passes the same declarative-only validator; Draft rejects commands, scripts, native code, executable permissions, links, unsafe paths, unknown contributions, and non-static assets.

Installing grants nothing. A package that declares commands is installed, enabled and serving its static contributions with those commands inert until they are authorized. `--grant` performs that second decision in the same invocation; it remains a separate, separately audited state change.

### `draft extension update <id> [--source <source-id>] [--version <version>] [--grant <permission>]` / `draft extension update --all`

Updates only from a currently usable trusted chain, and only from the source recorded when the package was installed. Another source publishing the same id cannot take over an installed package; changing source is an explicit uninstall and reinstall. Downloads are bounded, atomically cached, integrity checked, and promoted only after validation; the prior installed package is retained and restored after a failed promotion. Installed provenance remains durable if a source expires, becomes unavailable, or is removed.

An update always retires the authorization it replaces, including one that asks for exactly the permissions already approved: a grant is bound to one artifact, and a new version or new content is a different artifact.

### `draft extension authorize <id> --grant <permission>` / `draft extension revoke <id> [--permission <permission>]`

Authorizes or withdraws what an installed package may do. A grant records the extension id, its source and publisher, the package version and its content digest, and is durable and audited. `revoke` withdraws a capability without uninstalling or disabling anything: the package keeps running its static contributions.

The only permission defined today is `process.execute`, which lets Draft run the programs a package declares. Draft runs them itself, directly on the program and its arguments — never through a shell — under a cleared environment, a workspace-confined working directory and an enforced time limit.

### `draft extension list|show|uninstall|enable|disable`

Manages installed package state. `list` and `show` report declared permissions, what is currently authorized, and whether any capability is being withheld. Enablement activates declarative contributions only. Authoritative root revocation suppresses contributions and blocks re-enablement without deleting installation or provenance history. Draft never executes extension entrypoints.

### `draft extension tool list [--json]`

### `draft extension tool invoke <action-id> [--apply] [--json]`

Lists the tool actions installed extensions offer, or runs one. An action whose artifact has no grant to execute is listed as withheld rather than omitted, because "nothing offers this" and "something offers it and you have not authorized it" are different answers and only the second has a fix.

A tool returns findings and _proposed_ mutations, and stops there. Its response has nowhere to put an operation id, an actor, a precondition or a plan, so a package cannot supply them even by accident. Draft opens an edit session under its own operation id and attribution, stages each proposal — which is where protections, path safety and the workspace lease apply, identically to a human edit — and commits. Without `--apply`, the proposals are only reported.

A proposal Draft refuses stops the whole operation: applying the half it accepted would leave the project in a state neither the tool nor the user asked for. An action that proposes a mutation its declared effect does not permit is refused rather than downgraded to its findings.

### `draft resource state <res-id> [--json]`

One Resource's accepted state and what established it: the state digest the Baseline accepts, the provider provenance behind it, and what the workspace holds now. Both are shown because a reader given only one of the two cannot tell whether the Resource has moved.

### `draft resource observation show|coverage|provenance|pending|preview|adopt|transitions`

`show` prints the observation semantics **in force** — the ones this project adopted — not what the currently installed extensions would observe under. Those are different questions, and `pending` answers the second. Its digest is what labels a snapshot, and it deliberately excludes _which implementation_ observed, so a semantics-preserving upgrade does not force a re-baseline.

`coverage` prints the domains one observation covered, which resource belongs to which, and what it could not see. A domain that is `Incomplete` is why an addition or a removal Draft cannot prove is reported as a derivation gap rather than as a change.

`pending` prints the semantics an installed extension would observe under, if adopted — and prints nothing in the ordinary case, including a package update that changed nothing about what is observable. Installing, updating, disabling or removing anything whose effective observation semantics differ records a candidate; it never changes what is observed.

`preview` prints what adopting that candidate would do: which bindings it adds, drops or redefines, which resources would enter or leave the observed universe, and which work would be superseded. The trial observation behind it is thrown away — no snapshot, no provenance record, no change to what is in force. Looking at the consequences of a change must never be a way of making it.

`adopt` installs the candidate: a new baseline observed under the new semantics, a transition record, and the active pointer, all under the project lease and in one act. Work derived under the retired semantics stays readable but must be re-derived before it can be changed or promoted.

`transitions` lists every adoption this project has made.

`provenance` prints which implementation actually performed an observation, and may list **several** records for one observed state: the same state observed again later, or by a semantics-equivalent build, is a different historical observation, and a receipt that relied on the first keeps pointing at the first. Draft's own observer is recorded as a Core component and revision — never as a fabricated producer, attestation or grant.

### Capability gaps

Draft ships no extension packages, so a new installation manages resources and knows nothing about what they are. Commands never disappear: `draft pack evidence run` still runs, still honours the project's own `verify.toml`, still records evidence and still returns the same exit codes. What it adds is a capability gap naming the resources no installed extension could interpret, so the missing knowledge is visible rather than silently absent. Suggesting an installable package is best effort and never blocks: verification completes with no configured source, with a source disabled, offline, or with a refresh failing.

## Receipts And Storage

### `draft doctor receipts [<rcp-id>] [--all]`

Verifies durable receipts. Reading one is part of working on a ChangePack; _verifying_ one is a diagnosis, so it lives under `doctor`. Verification reports structure, canonical form, signature, historical trust at issuance and current trust separately — what cannot be determined reads `unknown`, never `valid`.

### `draft maintenance gc|compact|stats|index-rebuild|remove-project`

Maintains `.draft/` storage. Indexes, caches, and temporary data are rebuildable, and `gc` never deletes canonical history: it validates the accepted Baseline, preserves active and recoverable ChangePacks, removes safe temp and cache metadata, rebuilds indexes, and records maintenance events. `index-rebuild` is the transactional rebuild of the derived index from authoritative state. `remove-project` removes Draft metadata from the project without deleting project files, refusing pending unsafe state unless `--force` is given — and even then it leaves user files untouched. `draft doctor storage` reports without changing anything.
