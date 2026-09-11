# Baseline Protocol

The accepted **Baseline** is the exact historical node the project accepts. It
is not a branch, not a pointer to a working state, and not a summary. It
advances only through Promotion, on an approving Decision over a satisfied Gate.

```
BaselineManifest {
    project,
    project_state_root,       what material state is accepted
    state_evidence_root,      what exact provenance establishes it
    coverage_evidence_root,   what coverage claims justify absence
    parent_baseline_id,       single parent
    format_revision: 1,
}  →  BaselineId
```

`BaselineRecord` carries the acceptance metadata — `origin`, `actor`,
`accepted_at` — none of which is reachable from the manifest roots and none of
which affects `BaselineId`.

Baseline lineage is single-parent. A promotion states the Baseline it was
decided against; a promotion onto a parent that has since moved is **refused**
rather than rebased, because rebasing would be a decision about whether the two
changes compose, and nobody made it.

Baseline composition exposes `accepted_provider_provenance` — the binding and
semantic definition that produced each accepted primary Observation. It never
exposes a route, and it never changes when a binding is later reprofiled or
unbound. See [The Draft Change Graph](../../docs/reference/dcg.md).

Baseline metadata is compact and duplicates no Change payload. Immutable Change
manifests, revisions, evidence and trust records stay in their own canonical
stores; only mutable staging is ever disposed.
