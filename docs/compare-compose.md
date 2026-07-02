# Compare And Compose

Compare and compose help reviewers reason about multiple ChangePacks.

## Compare

```bash
draft compare <left-ChangePack> <right-ChangePack>
```

Compare reports:

- changed files in each ChangePack;
- files changed by both ChangePacks;
- compatibility summary;
- overlap warnings.

Text patches include stable hunk records. Compare reports same-file overlaps and hunk overlaps separately so same-file non-overlapping edits can be composed.

## Compose

```bash
draft compose <left-ChangePack> <right-ChangePack> --output "combined change"
```

Compose creates a new ChangePack from the source ChangePack patch data when the source ChangePacks are compatible. Draft rejects overlapping changes instead of guessing how to merge them.

## Review Checklist

Before composing:

- verify both source ChangePacks;
- inspect risk findings;
- compare for overlaps;
- ensure the combined change still has a coherent purpose;
- run verification on the composed ChangePack.

## Receipts And Events

Composition records a receipt and appends an event. The new ChangePack stores source ChangePack IDs so reviewers can trace provenance.
