# Changelog

All notable public changes to Draft are tracked here.

## v0.3.4

Draft v0.3.4 is the frozen pre-release contract cleanup. The displayed product,
Cargo, npm, and CLI version remains `0.3.4`; each independently persisted or
transmitted contract owns its schema policy and supports numeric
`schema_version: 1` in this release.

### Canonical contracts

- Contract membership is a compile-time-closed `ContractId` enum. Every production Rust type maps statically through `VersionedContract`, each entry owns independent current/supported policy, and arbitrary runtime registration or string-selected dispatch is forbidden.
- Every Draft-owned schema, filename, identifier, module, protocol, and API is stable and unversioned. IPC uses `protocol: "draft-ipc"`; Console HTTP intentionally uses `/api/v1/...`; Draftpack uses the format identifier `draftpack`.
- IPC, Console JSON bodies and responses, SSE data payloads, extensions, registries, operations, events, receipts, packs, revisions, lifecycle, quarantine, evidence, and archives require numeric `schema_version: 1`.
- Missing or malformed schema markers fail validation on wire input and report corruption for authoritative persisted state. Other numeric schema versions report an unsupported-schema error.
- Rust transport types own the generated IPC and Console JSON Schema and TypeScript contracts; drift checks compare generated temporary output with committed assets.
- Persisted and wire artifacts remain self-describing; registry metadata validates their declared version and never reinterprets existing bytes. Container-owned internal rows inherit that container version, while independently signed, hashed, copied, persisted, transmitted, or decoded members are separate contracts.
- `draft_version` is product/provenance metadata, not a compatibility gate. Extension `draft_api` remains a separate constraint against product API SemVer.

### Canonical domain state

- Immutable pack manifests are digest-bound to every immutable pack revision. Lifecycle, quarantine, verification, risk, review, decisions, rollback, and signed receipts remain separate records bound to exact revision digests.
- Draftpack archives embed content objects and authenticate the complete member set. Import validates the header, artifact digest, manifest, revision, lifecycle, provenance, evidence, and content objects before quarantine.
- Project and system event ledgers share one ledger-scoped, domain-separated hash chain. Signed receipts link to exact events and cannot be transplanted between workspace or system ledger identities.
- Canonical source digests depend only on normalized, ordered source entries and explicit semantic metadata. Workspace identity and ambient filesystem data do not influence the content digest.
- Durable operations, recovery details, fenced leases, and asynchronous jobs use one operation subsystem while retaining their distinct responsibilities.
- Extension provenance distinguishes verified HTTPS catalogs, explicitly trusted local catalogs, and direct user-authorized local packages.
- `user.name` and optional `user.email` are canonical project/global display/contact configuration. Project overrides global, missing name resolves to non-persisted `unknown`, empty values are rejected, and profile values never affect security identity, keys, trust, authorization, attribution, receipts, hashes, digests, or ownership.

### Architecture and safety

- `draft-core` is organized into `app`, `contracts`, `support`, `workspace`, `task`, `pack`, `review`, `trust`, `operation`, and `read_model` namespaces. Architecture checks enforce dependency direction, storage ownership, and the absence of UI styling and persisted models under orchestration modules.
- Browser Console ownership moved to `console/` (`console/web/` and `console/dist/`). It owns HTTP/browser transport, session security, DTOs, embedded assets, and presentation only; canonical domain behavior remains in `draftd`, services, and core. Intentional external AG-UI adapter support is unchanged.
- The standalone `draft identity` profile command and `identity.*` namespace were removed. Retired profile state fails closed without parsing or migration; Doctor/status recovery and `draft close` may only report, guide removal, or safely remove an unsupported workspace.
- This release is an intentional source- and data-compatibility cut. Draft does not parse, translate, repair, or rewrite earlier authoritative formats. Unsupported or corrupt bytes remain untouched.
- Fresh state initializes only when authoritative state is genuinely absent. Rebuildable indexes and caches regenerate only after canonical authoritative inputs validate successfully.
- Doctor reports unsupported schema, corruption, and validation failures as distinct categories and does not offer automated conversion or repair.
