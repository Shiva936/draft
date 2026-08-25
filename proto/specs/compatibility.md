# Compatibility Protocol

Draft v0.3.4 is a pre-release contract cut, not a state migration release.
Unsupported earlier authoritative formats are rejected without aliasing,
normalizing, applying, or rewriting their values.

Each independently persisted or transmitted boundary declares its own numeric
`schema_version` and maps statically to the closed Rust `ContractId` registry.
All contracts in v0.3.4 support only version `1`. A future decoder set or
version policy for one contract changes only that contract; new membership
requires an explicit `ContractId` code change. Runtime registration and
string-selected production dispatch are forbidden.

Containers such as canonical files, SQLite databases, archives, and envelopes
version their independently decoded boundary. Internal rows or subrecords
inherit a containing version when never independently encoded. Independently
hash-addressed, signed, copied, persisted, transmitted, or decoded members own
separate contract metadata.

Persisted/wire bytes are self-describing and are never reinterpreted from
registry metadata. `draft_version` records product provenance only. Workspace
compatibility is determined by the registered contracts actually present.
Extension `draft_api` matches the separately declared product API SemVer
`0.3.4`; it is not a schema version. Console `/api/v1/...` is the intentional
stable HTTP compatibility boundary.

Retired profile state is unsupported. Normal operations reject
`.draft/identity.json`, retired XDG profile files, `[identity]`, `identity.*`,
retired environment keys, and combined actor/profile fields without consuming
their values. Diagnostic/recovery operations may identify or safely remove the
unsupported workspace, never migrate it.
