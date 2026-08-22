# Draft v0.3.4 Release Notes

Draft v0.3.4 introduces task-centered local change orchestration on top of verified changepacks. Tasks carry deterministic scope, evidence, risk, and review contracts; candidate executions retain lineage and produce reviewable packs rather than mutating `stable_head`.

Finalization is now `draft submit`. New configurations, hooks, events, receipts, schemas, and output use submit terminology. Existing v0.3.x save-named data is read through versioned compatibility aliases. `draft doctor migrate` backs up the workspace metadata, project configuration, and every stored or quarantined pack manifest byte-for-byte, validates the complete transformed state before committing atomic replacements, and restores originals if a commit fails. The retired `draft save` command is not exposed.

The release adds evidence freshness, durable decisions, approval invalidation, expiring waivers, an attention inbox, submit-readiness checks, protected-file and redaction foundations, a global project registry, expanded Doctor maintenance, and user-scoped daemon foundations.

The browser and terminal review experiences are unified under Draft Console. Local extension packages retain versioned manifests, content hashes, atomic installation, and explicit enabled state, but Draft no longer executes extension entrypoints or bundles protocol bridges.

Safety invariants remain unchanged: `.draft/` never enters a change candidate, protected content is not logged or exported, failed submission preserves recoverable state, and only a fully successful `draft submit` may advance `stable_head`.

See [CHANGELOG.md](CHANGELOG.md) and the
[command reference](docs/reference/commands.md) for detailed behavior and
validation gates.
