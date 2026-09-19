# Composition Protocol

Composition asks one question about several sealed revisions: **can these be advanced separately, or do they have to be reasoned about together?**

It composes _revisions_, not ChangePacks. A ChangePack is an intention that can be resealed; a revision is what was actually sealed. Composing ChangePacks would let a reseal change what a composition claimed without the composition moving.

## The three answers

```
independent     nothing stands in the way
conflicting     both revisions touch a Resource, and nothing explains where
indeterminate   Draft cannot establish separability at all
```

`indeterminate` is not a hedge. `conflicting` is a claim Draft can defend — these two reach the same place. `indeterminate` is the honest answer when it cannot establish separability: revisions sealed from different Baselines describe different projects, and claim shapes Core has no way to relate cannot be compared at all. Both refuse composition, and only one of them says the work overlaps.

There is deliberately no `dependent` relationship and no topological ordering. What a revision was built on is its base Baseline, which `draft pack depends` reports from Baseline lineage; ordering a set of revisions by a declared dependency list would be a second, weaker answer to a question the accepted history already answers exactly.

## How a pair is decided

Where **both** sides recorded a representation, the conflict-claim algebra decides: a `Whole` claim admits no neighbours, two regions in the same coordinate space are compared as intervals, two keys in the same key space are compared as keys, and anything incomparable is `indeterminate`.

Where **either** side did not, Draft falls back to whole-Resource state: two revisions that both touch a Resource without explaining _where inside it_ cannot be shown separable, so they are reported as interfering rather than assumed composable.

The conservative direction always wins. Composing on a guess has a merge-shaped blast radius.

## The composition

```
Composition {
    schema_version, id, base_baseline,
    members: [ComposedRevision { change, revision, base_baseline, touched }],
    status: verified | failed,
    relations: [PairwiseRelation { left, right, relation, detail, shared_resources }],
    affected_resources,
    composition_digest,
}
```

A composition holds (`verified`) only when **every** pair is independent **and** every member was sealed from the `base_baseline` it names. The id is derived from the content, so composing the same revisions twice is the same composition rather than a second one to reason about.

Every non-independent relation carries a `detail` naming what stands in the way, so a reader can judge the claim rather than take a verdict on trust.

## Dispersal

`draft pack disperse` is the inverse, and is not a mutation. For each member it reports whether the member can be advanced on its own, and — when it cannot — the exact relations holding it. The answer says what to resolve rather than merely refusing.
