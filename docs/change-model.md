# Change model

Draft groups a provider-neutral `ProviderDelta` into `DraftChange`s — reviewable units that are **not** assumed to be commits.

- File changes carry path, optional old path (renames), status (added/modified/deleted/renamed/copied/type-changed/untracked/conflicted), add/delete counts, and a binary flag.
- Grouping is heuristic and path-based (tests, config, dependencies, migrations, docs, generated, binary, source). `GroupingSource` records whether grouping was automatic, manual, provider-suggested, or agent-suggested.
- Changes persist under `.draft/changes/change_<id>.json` with a `groups.json` index, surviving restarts.
