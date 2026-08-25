# Draft Console

Draft Console is a React and TypeScript application served by an authenticated loopback gateway. It is a separate transport into the same Draft operations as the CLI:

```text
React Console → authenticated loopback gateway → `draft-ipc` → draftd → Draft core/services → canonical state
```

`console/` owns the browser transport boundary: the Rust HTTP gateway, session and browser security, embedded assets, generated Console DTOs, and presentation behavior. `console/web/` contains the React source and `console/dist/` contains the reproducible embedded build. The gateway performs browser authentication, request validation, DTO translation, and event streaming only. It has no dependency on `draft-core`; domain validation, lifecycle transitions, valid actions, security identity checks, path safety, persistence, and canonical side effects remain `draftd`/service/core responsibilities. CI enforces this boundary with `scripts/check-console-architecture.sh`.

## Launching

Run from any directory:

```console
draft console
draft console --project ws_abc123 --port 4318
draft console --no-open --no-preselect
```

The command starts or reuses the global `draftd`. When run in a Draft project, it registers and preselects that immutable workspace id by default. The gateway binds only to `127.0.0.1` (or explicit `::1` through the library contract), never a wildcard or hostname-derived address.

## Browser session security

Each launch creates a random, memory-only, 60-second, single-use bootstrap secret. The CLI places it in the URL fragment, which is not sent with the initial HTTP request. The SPA POSTs it under exact Host, Origin, and fetch-metadata validation, then immediately removes the fragment with `history.replaceState`.

Successful exchange issues an opaque, short-lived, host-only cookie with `HttpOnly`, `SameSite=Strict`, no `Domain`, and `Path=/`. Mutations additionally require the session CSRF value and an `op_` operation id. The gateway rejects mismatched hosts, cross-site origins/fetch contexts, forwarding headers, oversized or slow requests, and unauthenticated SSE. CSP, no-store, referrer, and content-type protections apply to all responses.

The policy stays `default-src 'self'` with no `unsafe-inline`. Fonts are self-hosted under `font-src 'self'`, and each response carries a single-use CSP nonce that the served document exposes as `<meta name="csp-nonce">` so the editor can register its own stylesheet without relaxing `style-src`.

If `draftd` disconnects, reads return a structured `DAEMON_UNAVAILABLE` state and the UI offers retry. Restart with `draft service restart`; canonical project state is not stored in the browser or gateway.

Verification, submit, rollback, extension install/update, and other long-running work is submitted as a persisted `draft-job` contract (schema version `1` in v0.3.4) in the global operations namespace. Submission returns immediately. The Console polls authenticated job state, displays its current phase, and can request cancellation. Queued or interrupted jobs retain their original operation/correlation ids and resume when `draftd` restarts.

## Navigation and canonical state

System navigation is fixed to Overview, Projects, Inbox, Doctor, Extensions, and Settings. Project navigation is Overview, Tasks, Editor / Files, Events, and Packs. Pack actions remain inside pack context. Every route is keyed by opaque workspace and pack ids.

The top bar carries the project switcher, search, and a Create menu whose entries run real Draft flows: create task, create pack, new file, and register project. Each is disabled with its reason when there is no project context. The Inbox indicator counts unread notification records only. The sidebar collapses to an icon rail, adapts to a bottom tab bar on narrow widths, and reports live daemon reachability.

Every figure the Console shows comes from canonical Draft state. Where Draft records no value the view renders an explicit empty state rather than a zero, and a chart is omitted entirely when there is no metric behind it. Identity is shown as initials or a neutral mark; the Console has no profile images.

The browser stores only theme and display preferences. Projects, task metadata, Inbox/notification state, pack lifecycle, valid actions, extension state, registry records, operations, and editor attribution come from Draft state through `draftd`.

Settings has a Global and a Project scope. Global edits only `user.name` and `user.email`, and shows the stable security actor id, public-key id, global store location, and daemon state as read-only. Profile values are non-authoritative display/contact metadata and never affect signatures, trust, authorization, attribution, receipts, event hashes, ownership, or digests. An absent name is rendered through the non-persisted `unknown` fallback. Project scope edits canonical project configuration, ignore policy, hooks, and candidates.

Theme, colour accent, startup view, restored packs, sidebar state, and starred projects are display preferences stored in this browser alone; no other setting is cached client-side.

The Extensions view presents configured, trusted, and currently usable catalog state separately. Console accepts HTTPS source configuration and pasted out-of-band signed root metadata; it never accepts a source URL as trust and exposes no arbitrary filesystem browser. Expired discovery is read-only. Install, update, removal, update-all, and trust-root changes require contextual confirmation and execute through typed daemon operations. Local-directory catalogs and package paths remain explicit CLI workflows.

## Editor sessions

Opening a source file is read-only. Before saving, choose a task or pack attribution. `Ctrl/Cmd+S` persists a durable editor-session contract (schema version `1` in v0.3.4) and staged content below excluded `.draft/editor/`; it does not modify source files. Attribution cannot change inside an open session. File/folder creation, rename/move, and recursive deletion use that same staged session model and require a separate commit.

The editor opens files in tabs beside a collapsible file tree that marks canonical change state, and a file panel that carries metadata, task links, and the save, rename, move, new-file, and delete actions. Syntax highlighting for a file's language is fetched on demand rather than bundled into the initial payload.

`Commit to context` is explicit. Core revalidates workspace identity, the canonical base revision, protected control-directory paths, attribution, operation id, and a monotonically fenced workspace lease immediately before applying the journaled transaction. A conflicting source revision fails closed without changing the selected file. `.draft/` is unconditionally inaccessible, and control state such as `.git/` is protected by generic source policy.

Pack valid actions are computed by core. Reopening a verified, reviewing, approved, or rejected pack creates an audited next revision, resets current evidence bindings, and retains historical evidence. Submitted packs remain immutable; create a successor pack from their base instead.

## Global project identity and recovery

The project registry is an atomic registered contract in the platform-resolved global Draft store. Filesystem location is mutable metadata; `workspace_id` is immutable. Missing or damaged workspace identity is never regenerated silently.

Use `draft project relocate` only when the destination contains the same identity. A live copy with the same id blocks mutations at both paths. Use `draft project adopt-copy` to establish an independent identity; Draft first preserves the copied `.draft/` bytes and records source identity/digest and an adoption receipt.

The registry accepts only its canonical envelope and independently declared schema policy. Missing or malformed markers report corruption; another numeric schema reports unsupported schema. Doctor reports missing, moved, corrupt, unsupported, and conflicting projects independently and never rewrites authoritative bytes to permit startup.

## Keyboard and display

- `Ctrl/Cmd+K`, `/`, or `?`: open search/command palette
- `Esc`: close the active palette or dialog
- `Ctrl/Cmd+S`: persist the attributed editor session

Light, dark, and system themes are available, each generated from `console/web/src/theme/tokens.json` by `npm run tokens`; the emitted CSS custom properties are the only source of colour, spacing, radius, and type scale. The shell adapts to desktop, tablet, and mobile widths and uses visible focus, keyboard navigation, semantic tables, labels, live error states, reduced-motion support, and WCAG AA contrast in both themes.

Interface type is self-hosted Inter (SIL OFL 1.1) and icons are a checked-in subset of Lucide (ISC); both attributions live in `console/web/LICENSES.md`.
