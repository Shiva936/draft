# Composition Protocol

Active pack composition validates selected packs against the current stable
head, builds a dependency graph, topologically orders dependencies, rejects
cycles, rejects conflicts, and emits a deterministic `composition_hash`.

Relationships are `independent`, `dependent`, or `conflicting`.
