# Pack Protocol

Draft's primary work concept is a **Pack**, and the Pack family has exactly two members:

```
ChangePack    cpk_   project-local governable work lineage
RevisionPack  rpk_   immutable exact revision of a ChangePack
```

A ChangePack is never project state. What the project accepts advances only through Promotion, which requires an approving Decision citing a satisfied Gate over the exact RevisionPack being promoted.

## Identity

Both families are derived, never random: `cpk_` + the first 24 hex of `sha256("{base_baseline}|{intent}")`, so re-running the same request converges on the same ChangePack; and `rpk_` + the first 24 hex of `sha256("{cpk_}|{project_state_root}")`, so a RevisionPack depends on its ChangePack and the proposed state root and nothing else.

<!-- retired-architecture-ok: naming the retired families is how the spec says they fail closed. -->

The retired `chg_` and `rev_` families are not parsed.

## The work graph

```
ChangePack             generation, current_definition, ChangePackLifecycle
ChangePackDefinition   what this ChangePack is for and may touch; amended under CAS
ScopeResolution        resolved ONCE against an exact base Baseline, verified at seal
RevisionPack           one exact sealed state, immutable, create-once
```

The owning fields are typed and exact: `ChangePackDefinition.change_pack`, `ScopeResolution.change_pack` and `RevisionPack.change_pack` carry a `ChangePackId`, and every governance fact binds `revision_pack: RevisionPackId`. Cross-layer identifiers (DTOs, schemas, IPC, CLI) are `change_pack_id` / `revision_pack_id`.

`ScopeResolution` is resolved once and verified again at seal. A definition amended after resolution leaves the resolution stale rather than silently widening what the work may touch.

## Lifecycle

```
Active  → Completed   deterministic finalization inside the Promotion commit
        → Abandoned   "we tried this and stopped" — history is retained
        → Reopened    from Abandoned
```

There is no `delete`. Abandoning retains the ChangePack, its definitions, its RevisionPacks and everything recorded about them: "we tried this and stopped" is frequently the most useful thing in a project's history.

ChangePack mutations serialize through `ChangePackStore` under a `MutationJournal`, with `Absent` as the expected state for creation. Two amendments racing means the loser is rejected, not merged.

## What binds to a RevisionPack

Evidence, Assessments, Representations, Reviews, Decisions, Gate waivers and Gate evaluations each bind **one exact** `RevisionPackId` and never carry to another. The id is backed by a create-once digest binding, so "the same RevisionPack" means the same bytes rather than the same string: sealing identical content again converges, and different content under one id is an integrity violation.

## Storage

```
.draft/packs/change/<cpk_>/     ChangePack-owned content: manifest, content revisions, lockfile
.draft/packs/revision/          sealed RevisionPack facts
.draft/graph/change-packs/      the authoritative, CAS-guarded ChangePack record
```

A ChangePack's _content revisions_ (`ChangePackContentRevisionRecord`, labelled `content_initial`, …) and its descriptive _review progress_ (`review-progress.json`) are content-store vocabulary, not RevisionPacks. Review progress is derived one way from authoritative ChangePack and governance state and is never an authority over lifecycle, Baselines or Promotion.

## Path safety

Every path-bearing payload rejects `.draft/`, absolute paths, traversal, symlink escapes, and any path outside the project root. See [Path Safety](path-safety.md).
