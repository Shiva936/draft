# Policy

Policy controls whether a ChangePack can be saved.

## Default Gates

The default v0.3.1 policy requires:

- verification before save;
- approval before save;
- blocking failed verification;
- human approval for high-risk changes.

## Policy Inputs

Policy uses:

- ChangePack status;
- verification results;
- risk findings;
- review decisions;
- save candidate safety checks;
- configured local options.

## Blocking Conditions

Draft blocks save when:

- verification is required and missing;
- verification failed and blocking is enabled;
- approval is required and missing;
- risk requires human approval and none is present;
- `.draft/` appears in the save candidate.

The `.draft/` save-candidate block is not configurable.

## Receipts

Policy decisions that affect save should be visible through events and receipts. Failed saves record failure reasons so maintainers can diagnose why an action did not run.

## Project Policy

Projects can tighten policy over time. Policy changes should be reviewed because they affect what Draft allows to be saved.
