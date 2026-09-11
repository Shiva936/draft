# Draft Console

Draft Console has browser and terminal frontends over the same authoritative daemon state:

```text
React Console → authenticated loopback gateway → `draft-ipc` → draftd → Draft core/services → canonical state
Terminal Console → console/application client ────────┘
```

`console/` owns the browser transport boundary: the Rust HTTP gateway, session and browser security, embedded assets, generated Console DTOs, and presentation behavior. `console/web/` contains the React source and `console/dist/` contains the reproducible embedded build. The gateway performs browser authentication, request validation, DTO translation, and event streaming only. It has no dependency on `draft-core`; domain validation, lifecycle transitions, valid actions, security identity checks, path safety, persistence, and canonical side effects remain `draftd`/service/core responsibilities. CI enforces this boundary with `scripts/check-console-architecture.sh`.

## Launching

Run from any directory:

```console
draft console web
draft console web --project ws_abc123 --port 4318
draft console web --no-open --no-preselect
draft console tui
draft console tui --project ws_abc123
```

The command starts or reuses the global `draftd`. When run inside an already-registered project, it preselects that immutable workspace id by default; it never registers a workspace implicitly. `--no-preselect` starts in GLOBAL context. The web gateway binds only to `127.0.0.1` (or explicit `::1` through the library contract), never a wildcard or hostname-derived address.

The terminal frontend uses `console/application/`, which first completes the existing Draft IPC handshake and then negotiates the separately versioned Console application protocol. On disconnect it keeps previously loaded models visibly stale and invalidates all domain action descriptors.

Both frontends act through the same authoritative action model. An action arrives from `draftd` with its label, whether it is currently available and why not, and the inputs it needs — free text, a choice from a server-supplied set, a flag, or an explicit acknowledgement. Each input has a stable machine id separate from the label a person reads, so relabelling or translating one changes nothing about what may be submitted. `draftd` validates every argument against the contract the action declared and rejects anything undeclared, missing, wrongly typed, or outside its own option set; a frontend's own checks are a convenience only. The descriptor a client holds is bound to that action, its target, the authoritative revision and a digest of those inputs, so acting on a stale option set conflicts instead of proceeding.

This is what lets extension management work identically in both frontends. Install, update, enable, disable, authorize, revoke and the catalog-source operations are offered on the system scope, targeted at the extension or source they act on, and run through the same daemon operations the CLI uses. Neither frontend contains an extension-specific form; an action a future extension contributes renders with no frontend change.

The browser reaches that model through `/api/v1/console/model` and invokes through `/api/v1/console/actions/invoke`. Its Extensions screen looks actions up by stable action id and target id, so a control appears only because Draft issued one and is available only because Draft says so; raw extension and source state is still displayed as status and decides nothing. The existing extension routes are unchanged and remain available to existing callers as compatibility adapters over the same operations.

The Console application session belongs to the gateway, not the browser. It is established once per browser session, replaced at most once per generation when Draft no longer recognises it, and never exposed as a request field. Invocation capabilities are short-lived, are never stored in the browser, and a mutation is never replayed after an ambiguous failure — after any successful action the Console refetches the authoritative model rather than predicting what the next available actions will be.

## Browser session security

Each launch creates a random, memory-only, 60-second, single-use bootstrap secret. The CLI places it in the URL fragment, which is not sent with the initial HTTP request. The SPA POSTs it under exact Host, Origin, and fetch-metadata validation, then immediately removes the fragment with `history.replaceState`.

Successful exchange issues an opaque, short-lived, host-only cookie with `HttpOnly`, `SameSite=Strict`, no `Domain`, and `Path=/`. Mutations additionally require the session CSRF value and an `op_` operation id. The gateway rejects mismatched hosts, cross-site origins/fetch contexts, forwarding headers, oversized or slow requests, and unauthenticated SSE. CSP, no-store, referrer, and content-type protections apply to all responses.

The policy stays `default-src 'self'` with no `unsafe-inline`. Fonts are self-hosted under `font-src 'self'`, and each response carries a single-use CSP nonce that the served document exposes as `<meta name="csp-nonce">` so the resource viewer can register its own stylesheet without relaxing `style-src`.

If `draftd` disconnects, reads return a structured `DAEMON_UNAVAILABLE` state and the UI offers retry. Restart with `draft daemon restart`; canonical project state is not stored in the browser or gateway.

Verification, promotion, publication, recovery, extension install/update, and other long-running work is queued as a persisted `draft-job` contract (schema version `1` in v0.3.4) in the global operations namespace. Submission returns immediately. The Console polls authenticated job state, displays its current phase, and can request cancellation. Queued or interrupted jobs retain their original operation/correlation ids and resume when `draftd` restarts.

## Navigation and canonical state

System navigation is fixed to Overview, Projects, Inbox, Doctor, Extensions, and Settings.

Project navigation is **Overview, Work, Resources, Baselines, Activity, Providers, and Extensions**, and it nests:

| Section | Views | Why here |
| --- | --- | --- |
| Overview | — | Condensed project status. It summarises the sections below; it does not reimplement them. |
| Work | Tasks · Changes | The two things a person does _to_ a project. |
| Resources | Resources · Observation | An observation is evidence _about_ a Resource, so it lives inside the Resource domain. |
| Baselines | Baselines · Publications | What the project accepts, and separately what was delivered from it. |
| Activity | — | The append-only Activity Ledger projection. |
| Providers | — | Bindings, semantic definitions and operational profiles. |
| Extensions | Extensions · Tools | A tool exists only because an extension contributes it. |

A Change has its own scope with fourteen views — Summary, Intent, Scope, Revisions, Impact, Representations, Evidence, Assessments, Review, Decisions, Gates, Promotion, Receipts, Recovery — one per act, because each is a separate fact. A reader who cannot tell an approval from a passing check cannot tell what authorized a promotion. A Baseline has its own scope too: Summary, State root, Evidence root, Coverage, Lineage, Composition, Recoverability, Receipts, Publications.

This structure is not written in the frontends. `draftd` serves it from one definition in the Console application protocol, and the browser's copy is generated from that same definition — so the sections a frontend renders cannot drift from the sections the authority offers.

The Changes view names the revision's representation alongside its evidence and assessments: how many Resources were explained and by which strategy, because "explained by the neutral rendering" and "explained by an installed extension" are different facts, and only one of them says where inside a Resource the work landed. Every route is keyed by opaque project, Change and Baseline ids.

The top bar carries the project switcher, search, and a Create menu whose entries run real Draft flows: create task, create Change, new file, and register project. Each is disabled with its reason when there is no project context. The Inbox indicator counts unread notification records only. The sidebar collapses to an icon rail, adapts to a bottom tab bar on narrow widths, and reports live daemon reachability.

Every figure the Console shows comes from canonical Draft state. Where Draft records no value the view renders an explicit empty state rather than a zero, and a chart is omitted entirely when there is no metric behind it. Identity is shown as initials or a neutral mark; the Console has no profile images.

The browser stores only theme and display preferences. Projects, task metadata, Inbox/notification state, revision state, valid actions, extension state, registry records, operations, and edit attribution come from Draft state through `draftd`.

Settings has a Global and a Project scope. Global edits only `user.name` and `user.email`, and shows the stable security actor id, public-key id, global store location, and daemon state as read-only. Profile values are non-authoritative display/contact metadata and never affect signatures, trust, authorization, attribution, receipts, event hashes, ownership, or digests. An absent name is rendered through the non-persisted `unknown` fallback. Project scope edits canonical project configuration, ignore policy, hooks, and candidates.

Theme, colour accent, startup view, restored Changes, sidebar state, and starred projects are display preferences stored in this browser alone; no other setting is cached client-side.

The Extensions view presents configured, trusted, and currently usable catalog state separately, along with whether each source is enabled and when it was last refreshed. Console accepts HTTPS source configuration and pasted out-of-band signed root metadata; it never accepts a source URL as trust and exposes no arbitrary filesystem browser. Expired discovery is read-only. Install, update, removal, enable/disable, update-all, authorization changes, and trust-root changes require contextual confirmation and execute through typed daemon operations. Local-directory catalogs and package paths remain explicit CLI workflows.

Discovery reads verified cached catalog metadata, so the view works offline; contacting a source is a separate, explicit refresh. Installing is presented as granting nothing — a package that declares commands appears as installed and enabled with those commands inert, and authorizing them is its own action showing exactly what is requested. Because a grant is bound to one artifact, an updated package shows as needing authorization again. Disabling a source stops discovery and updates from it without touching what it already installed; a source built into the Draft build is disabled rather than removed.

## Resource sessions

A resource's type is never guessed from its name. Draft reports the **classes** an installed extension assigns it, and the Console renders those; with nothing installed a resource is simply unclassified and still fully listable, openable and editable.

Classes are a list, not a single value, because a resource genuinely is several things at once — a Rust file is both `draft.text.document/document` and `draft.language.rust/source`, and showing one would discard a correct assignment. Where two publishers define the _same_ class incompatibly, that class is withheld and reported as disputed; every other class on the resource still stands.

A resource's panel also names the **presentation** Draft resolved for it. Bindings are chosen by specificity; where two publishers bind equally the result is reported as ambiguous and left for a person to settle, because silently picking one would make a publisher's choice look like Draft's. Where nothing binds, the universal neutral presentation renders the resource's intrinsic facts, so a resource from a domain nothing understands is still readable.

Syntax highlighting follows that resolved binding: a `text_editor` presentation names a grammar in its config, and the Console loads the matching asset. The vocabularies stay on their own sides — an extension says which grammar its resources want, and the Console owns what a grammar _is_, exactly as it owns what `text_editor` is. Adding a grammar is additive Console work and changes no domain model. A binding that names a grammar this build does not have, and a resource with no resolved binding, both render as plain text. There is no filename fallback: guessing a language from a suffix is exactly the domain knowledge that belongs in an extension, and an unhighlighted resource is correct where a wrongly highlighted one is not.

Opening a resource is read-only. Before saving, choose a task or Change attribution. `Ctrl/Cmd+S` persists a durable Workspace contract (schema version `1` in v0.3.4) and staged content below excluded `.draft/workspaces/`; it does not modify project state. Attribution cannot change inside an open session. Creation, relocation and recursive removal use that same staged session model and require a separate commit.

The browser opens resources in tabs beside a collapsible tree that marks the neutral change aspects, and a panel carrying state, classes, task links, and the save, relocate, new-resource and delete actions. A grammar is fetched on demand rather than bundled into the initial payload.

The tree groups `file`-scheme resources hierarchically, because that adapter's locator bodies genuinely are paths. Any other scheme is grouped under its scheme name and listed whole: a catalog entry or a timeline event has no containing folder, and the Console never splits a locator body it does not own.

`Commit to context` is explicit. Core revalidates workspace identity, the canonical base revision, protected control-directory paths, attribution, operation id, and a monotonically fenced workspace lease immediately before applying the journaled transaction. A conflicting source revision fails closed without changing the selected file. `.draft/` is unconditionally inaccessible — that is Draft's own control plane and the one protection Draft owns. Any other directory an external tool keeps its state in is outside the observed universe only when an installed view rule says so; with nothing installed such a directory is ordinary project state, because Draft does not know what those tools are.

Which actions are valid is computed by Core and is presentation, never authorization. Sealing a new revision of an active Change is an ordinary next step; nothing recorded about an earlier revision is invalidated, because Evidence, Assessments, Gates and Decisions each bind one exact revision. A completed Change is immutable — create a successor Change instead.

## Providers

**Providers** is how this project is attached to the systems that observe it, and it keeps three kinds of fact apart because their mutability differs.

A **binding** is mutable: it can be unbound and rebound, and its generation moves when it does. A **semantic definition** and an **operational profile** are immutable, content-addressed facts a binding points at. That is why unbinding destroys nothing — the definition an accepted Baseline was composed under is still there and still readable afterwards, which is what lets `draft baseline composition` keep answering after a binding has moved on.

Two acts are offered from the Console: **Unbind**, which stops new work routing through a binding, and **Rebind**, which resumes it. Both run the same audited application operation the CLI calls, so a Console unbind is journalled, audited and appended to Activity exactly like a terminal one. Binding, redefining and reprofiling are not offered here: each needs a canonical semantics contract, definition or profile document, and authoring one belongs to `draft project provider bind`, where the input syntax lives.

Providers are not Extensions. An extension contributes capability to Draft; a provider binding attaches this project to something that observes it. They are separate sections because conflating them would suggest that installing something could change what established a Baseline.

## Observation and tools

Both live inside the sections they belong to — Observation under Resources, Tools under Extensions.

**Observation** reports what the current observation established, in two panels that are deliberately not merged.

_Coverage_ lists the domains the observation covered and whether each is complete. A domain is named by the pair `(adapter binding, its own domain id)` and never by the local half alone: the local id is opaque, and two adapters may both call a domain `root` and mean unrelated things. Where a domain is incomplete, the gaps that made it so are shown in the adapter's own words — Draft does not paraphrase them, and never describes a domain as a folder or subdirectory, because no scheme is obliged to have a hierarchy. Absence inside a complete domain is authoritative; absence inside an incomplete one proves nothing, which is why a change set reports uncertainty there instead of a deletion.

_Observation history_ records which implementation actually looked. It is a list, not a record: the same state observed again later is a _different_ historical observation, and a receipt that relied on the first keeps pointing at the first. Each run shows what it attempted separately from what it committed, so a failed first attempt and its successful retry are both visible rather than the retry erasing the failure. Draft's own observer is shown as a Core component and revision; only an extension observer carries a producer, an attestation and an authorization decision, and the view never describes one in the other's terms.

Neither panel says anything about whether a state could be put back. Observability and restorability are separate capabilities with separate evidence, and coverage never implies recovery.

**Tools** lists the actions installed extensions contribute, with the resources each matches, the effect it declared, and whether its artifact is authorized to run. An action nobody has authorized is listed as withheld rather than hidden: "nothing offers this" and "something offers it and you have not authorized it" are different answers, and only the second has a fix.

Running a tool never changes the project. The tool returns findings and _proposed_ mutations and stops there, so previewing and applying are two separate decisions. Applying opens a Draft edit session under Draft's own operation id and attribution, where protections, path safety and the workspace lease apply exactly as they do to a person's edit. A proposal Draft refuses stops the whole operation rather than applying the half it accepted.

## Global project identity and recovery

The project registry is an atomic registered contract in the platform-resolved global Draft store. Filesystem location is mutable metadata; `workspace_id` is immutable. Missing or damaged workspace identity is never regenerated silently.

Use `draft project relocate` only when the destination contains the same identity. A live copy with the same id blocks mutations at both paths. Use `draft project adopt-copy` to establish an independent identity; Draft first preserves the copied `.draft/` bytes and records source identity/digest and an adoption receipt.

The registry accepts only its canonical envelope and independently declared schema policy. Missing or malformed markers report corruption; another numeric schema reports unsupported schema. Doctor reports missing, moved, corrupt, unsupported, and conflicting projects independently and never rewrites authoritative bytes to permit startup.

## Keyboard and display

- `Ctrl/Cmd+K`, `/`, or `?`: open search/command palette
- `Esc`: close the active palette or dialog
- `Ctrl/Cmd+S`: persist the attributed resource session

Light, dark, and system themes are available, each generated from `console/web/src/theme/tokens.json` by `npm run tokens`; the emitted CSS custom properties are the only source of colour, spacing, radius, and type scale. The shell adapts to desktop, tablet, and mobile widths and uses visible focus, keyboard navigation, semantic tables, labels, live error states, reduced-motion support, and WCAG AA contrast in both themes.

Interface type is self-hosted Inter (SIL OFL 1.1) and icons are a checked-in subset of Lucide (ISC); both attributions live in `console/web/LICENSES.md`.
