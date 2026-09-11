# Change Protocol

A Change is a unit of proposed work in the Draft Change Graph. Canonical
identifiers use `chg_<id>`; a sealed revision of one uses `rev_<id>`.

A Change is never project state. What the project accepts advances only through
Promotion, which requires an approving Decision citing a satisfied Gate over the
exact revision being promoted.

## The work graph

```
Change              generation, current_definition, ChangeLifecycle
ChangeDefinition    what this Change is allowed to touch; amended under CAS
ScopeResolution     resolved ONCE against an exact base Baseline, verified at seal
ChangeRevision      one exact sealed state, immutable, create-once
```

`ScopeResolution` is resolved once and verified again at seal. A definition
amended after resolution leaves the resolution stale rather than silently
widening what the work may touch.

## Lifecycle

```
Active  → Completed   deterministic finalization inside the Promotion commit
        → Abandoned   "we tried this and stopped" — history is retained
        → Reopened    from Abandoned
```

There is no `delete`. Abandoning retains the Change, its definitions, its sealed
revisions and everything recorded about them: "we tried this and stopped" is
frequently the most useful thing in a project's history.

Change mutations serialize through `ChangeStore` under a `MutationJournal`, with
`Absent` as the expected state for creation. Two amendments racing means the
loser is rejected, not merged.

## What binds to a revision

Evidence, Assessments, Representations, Reviews, Decisions, Gate waivers and
Gate evaluations each bind **one exact** `ChangeRevisionId` and never carry to
another. The revision id is backed by a create-once digest binding, so "the same
revision" means the same bytes rather than the same string.

## Path safety

Every path-bearing payload rejects `.draft/`, absolute paths, traversal, symlink
escapes, and any path outside the project root. See
[Path Safety](path-safety.md).
