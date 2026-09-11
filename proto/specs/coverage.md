# Coverage Protocol

Absence has to be proved, not assumed. A Resource with no established state does
not appear in `ProjectStateRoot`; what justifies its absence is
`CoverageEvidenceRoot`.

The failure this exists to prevent is specific: a project observed by a provider
that silently returned nothing looks, from the state root alone, exactly like a
project that genuinely has nothing. One of those is a complete answer and the
other is no answer at all.

## CoverageEvidence

```
CoverageEvidence {
    provider_binding, provider_semantic_definition,
    domain: CoverageDomainRef,
    status: CoverageStatus,            // Complete | Incomplete | NotObserved
    observation_run: Option<ObservationRunRef>,
    attempted: bool,
    committed: bool,
    known_gaps: BTreeSet<ObservationGapRef>,
}
```

<!-- retired-architecture-ok: naming the omitted field is the point. -->
There is **no `completeness_proof` field in v1**. An undefined proof is not a
proof, and a field that every producer fills with something plausible is worse
than an honest absence.

## Cross-field validity

| `status` | `attempted` | `committed` | `observation_run` | `known_gaps` |
|---|---|---|---|---|
| `Complete` | `true` | `true` | `Some(..)` | **empty** for the claimed domain |
| `Incomplete` | `true` | per the repository model | `Some(..)` | **non-empty** |
| `NotObserved`, nothing attempted | `false` | `false` | **`None`** | may be empty |
| `NotObserved`, attempt failed | `true` | `false` | `Some(..)` | may carry the failure gap |

Any other combination is a **hard construction error**, not a warning.

`attempted == false` with `Some(run)` is rejected outright: there is no
synthetic run for an attempt that never happened, and fabricating one would make
"we did not look" indistinguishable from "we looked and found nothing".

`attempted ≠ committed` is preserved. A failed attempt can never be promoted to
`Complete` by any later step.

## Canonical construction

- Sort by `(provider_binding, provider_semantic_definition, domain)`.
- Exact duplicates are idempotent.
- A duplicate key with a differing status, run or gap set is a hard error.
- Gaps sort by `ObservationGapRef`.
- The empty collection has a defined domain-separated empty-root constant.
- Chunking, leaf and interior encodings, and domain separators are frozen.

## Coverage is not material state

Coverage never enters `ProjectStateRoot`. Stronger coverage over identical
material state yields:

```
the SAME ProjectStateRoot
a DIFFERENT CoverageEvidenceRoot
and therefore a DIFFERENT BaselineId
```

That is deliberate. The project accepts a different historical node because it
knows more about the same state. Two Baselines can agree exactly on what is
there and disagree on how much of it was actually looked at, and both facts
belong in accepted history.

## Reading it

`draft baseline coverage` renders per-provider, per-domain coverage and
distinguishes *not observed (no attempt)* from *not observed (attempt failed)*.

**No user-facing text may imply that an empty Resource set proves complete
observation.** The Console applies the same rule: it never infers absence from
an empty Resource list.
