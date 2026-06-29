# Compare And Compose

Compare and compose help reviewers reason about multiple changepacks.

## Compare

```bash
draft compare <left-pack> <right-pack>
```

Compare reports:

- changed files in each pack;
- files changed by both packs;
- compatibility summary;
- overlap warnings.

Text patches include stable hunk records. Compare reports same-file overlaps and hunk overlaps separately so same-file non-overlapping edits can be composed.

## Compose

```bash
draft compose <left-pack> <right-pack> --output "combined change"
```

Compose creates a new changepack from the source pack patch data when the source packs are compatible. Draft rejects overlapping changes instead of guessing how to merge them.

## Review Checklist

Before composing:

- verify both source packs;
- inspect risk findings;
- compare for overlaps;
- ensure the combined change still has a coherent purpose;
- run verification on the composed pack.

## Receipts And Events

Composition records a receipt and appends an event. The new pack stores source pack ids so reviewers can trace provenance.
