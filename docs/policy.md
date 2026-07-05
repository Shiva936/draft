# Policy

Policy controls whether a ChangePack can be verified, approved, and saved.

## Resolution and Precedence

The effective policy is resolved field by field, highest precedence first:

1. project policy: `<root>/.draft/policy.toml`
2. global default policy: `~/.draft/policies/default-policy.toml`
3. Draft's built-in safe default

A key present in a higher layer overrides that key only; unspecified keys fall
through to the next layer. A policy file that exists but cannot be read or
parsed fails closed: the gated operation aborts instead of silently falling
back to a more permissive layer.

## Canonical Policy Keys

| Key | Default | Enforced at |
| --- | --- | --- |
| `require_approval_for_save` | `true` | save gate |
| `block_on_critical_risk` | `true` | save gate (also blocks when no risk report exists) |
| `require_approval_on_high_risk` | `true` | save gate |
| `require_reverify_on_workspace_change` | `true` | save gate |
| `require_local_verify_for_imports` | `true` | import save gate |
| `require_full_verify_intents` | `["security", "migration"]` | `draft verify` (escalates to `--full`) |
| `require_fuzz_intents` | `["security"]` | `draft verify` (escalates to `--fuzz`) |

Legacy `[save]`/`[approval]`/`[agent]` tables in the same `policy.toml` are
still honored by the legacy save gates; canonical keys are top-level.

## Default Gates

The default v0.3.2 policy requires:

- verification before save;
- approval before save;
- no unresolved critical risk (a missing canonical risk report also blocks);
- re-verification when the workspace changed since verification;
- local re-verification of imported packs before save;
- full verification (and fuzzing) for `security`-intent packs;
- valid event, receipt, and transparency evidence for trust-relevant actions.

## Blocking Conditions

Draft blocks save when:

- verification is required and missing;
- verification failed and blocking is enabled;
- approval is required and missing;
- the canonical risk report is missing or reports `critical` risk;
- risk requires human approval and none is present;
- the workspace changed after verification;
- an imported pack has not been locally re-verified and approved;
- `.draft/` appears in the save candidate.

The `.draft/` save-candidate block is not configurable.

## Receipts

Policy decisions that affect save should be visible through events and receipts. Failed saves record failure reasons so maintainers can diagnose why an action did not run.

## Project Policy

Projects can tighten policy over time. Policy changes should be reviewed because they affect what Draft allows to be saved.
