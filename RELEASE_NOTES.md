# Draft v0.3.4 Release Notes

Draft v0.3.4 freezes the first canonical pre-release contracts. The product and package version remains `0.3.4`; every independently persisted or transmitted Draft boundary owns registered metadata and supports numeric `schema_version: 1` in this release.

This is an intentional compatibility cut. Earlier authoritative data and wire formats are rejected without fallback parsing, conversion, repair, or source rewrites. Missing or malformed schema markers are validation errors on wire input and corruption errors in persisted state. A different numeric schema is reported as unsupported. Fresh state is created only when authoritative state is genuinely absent.

The release establishes:

- a compile-time-closed typed contract registry with independently evolvable policies and stable unversioned IPC, Console, extension, ChangePack, archive, registry, Activity, receipt, operation, task, evidence, and configuration names;
- `/api/v1/...` Console routes with generated response envelopes and request/SSE versions derived from their registered contract metadata;
- the `draft-ipc` protocol identifiers;
- the Draft Change Graph: Resources and Relations, ChangePacks and sealed RevisionPacks, Evidence, Assessments, Reviews, immutable Decisions and Gates, a journalled Promotion into an immutable Baseline, and a separately identified Publication lifecycle;
- one Activity Ledger — framed, hash-chained, serialized by a correctness lock, with `events/events.log` the sole authoritative file, a closed v1 vocabulary where every event names an audit-fact owner and a journal mechanism, and exactly one converter and one appender;
- receipts that attest a Promotion, a publication outcome or an authorized resolution of one, stored create-once under a preallocated `rcp_` id and verified at three levels reported separately;
- ledger-scoped record hashing, so a record cannot be transplanted between one project's Activity and another's;
- deterministic canonical source views whose content digests exclude workspace identity and ambient filesystem metadata;
- one durable operation subsystem with distinct recovery, lease, and job data;
- typed HTTPS-catalog, trusted-local-catalog, and direct-local extension provenance;
- public core domain namespaces and enforced dependency/ownership checks;
- Rust-owned generation of IPC JSON Schema, Console HTTP JSON Schema, and TypeScript transport types.
- browser Console ownership under `console/`, with `console/web/` source and reproducible `console/dist/` assets, while all domain behavior remains in `draftd`/services/core;
- canonical optional `user.name` and `user.email` display/contact configuration, strictly isolated from stable security actor, key, trust, authorization, attribution, receipt, hash, digest, and ownership semantics.

`draft_version: 0.3.4` is product/provenance metadata, not a workspace compatibility gate. Compatibility follows the registered contracts present. Extension `draft_api` remains a separate product SemVer constraint.

Safety remains fail-closed: `.draft/` never enters a source view or artifact, corrupt authoritative bytes remain untouched, every immutable fact is bound create-once to the digest of its own canonical bytes and verified on load, and derived indexes rebuild only after their authoritative inputs validate.

See [CHANGELOG.md](CHANGELOG.md), the [protocol index](docs/internals/protocol.md), and the [command reference](docs/reference/commands.md) for the frozen contract.
