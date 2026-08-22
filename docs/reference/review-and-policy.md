# Review, Verification, And Policy

Draft separates evidence collection, risk analysis, review, and final approval so users can inspect why a ChangePack is or is not ready to submit.

## Review And Approval

### Review

```bash
draft review -p <ChangePack>
draft review -p <ChangePack> --comment "looks good"
draft review -p <ChangePack> --tui
```

Review records that the ChangePack entered the human review boundary. Inspect:

- changed files and patch content;
- verification output and evidence;
- risk findings and policy blockers;
- receipts and prior decisions.

### Approval And Rejection

```bash
draft approve -p <ChangePack> --reason "verified locally"
draft reject -p <ChangePack> --reason "needs changes"
```

Default policy requires approval before submit, and high-risk changes require human approval when that policy is enabled. Approval is local Draft metadata, not a hosted code-review approval.

## Draft Console

`draft review --tui` opens the terminal form of Draft Console. It is designed for repeated review work where a user needs to inspect status, evidence, risk, policy, approvals, receipts, and rollback options without leaving Draft.

The first view is summary-first: overview, hotspots, evidence gaps, provenance, readiness, and available actions. Semantic impact derived from LSIF and risk evidence appears before raw diff details.

Console sections cover:

- workspace status and latest scan time;
- ChangePack list, selection, file changes, and overlap indicators;
- verification and submit-readiness counts;
- risk findings, evidence gaps, and policy blockers;
- decisions and approve or reject actions;
- compare and compose actions;
- submit readiness, receipts, and rollback;
- service connection state.

The TUI uses the same core state and human-final checks as the CLI. It works without a daemon for static review; when `draftd` is available it can provide live refresh, background verification, and service status. The interface must not hide policy blockers, and failed submit receipts must remain visible.

## Verification

Verification runs local commands and records their results as Draft evidence.

```bash
draft verify -p <ChangePack-id-or-name>
```

Draft loads the selected local profile, runs checks from the workspace root, and captures stdout, stderr, exit code, and timing in a verification receipt. Results can be passed, failed, skipped, or errored. Policy can block submit when verification is missing or failed.

Verification commands are opaque local shell commands. Good checks are deterministic, local, scoped to the change, non-zero on failure, and concise enough to review. Results attach to the ChangePack, and submit receipts retain the references needed to reconstruct why it was allowed or blocked.

When verification fails:

1. Inspect the receipt and its stdout and stderr objects.
2. Fix the workspace.
3. Create or update the ChangePack.
4. Run verification again.

## Risk Engine

`draft risk` is deterministic and local. It returns a score, band, policy decision, stable reason codes, hotspots, evidence gaps, and a receipt.

Default rules cover sensitive paths, authentication and security files, payments, database migrations, dependency lockfiles, CI/CD files, container files, deleted tests, binary changes, large changes, and missing verification.

`.draft/risk.toml` can tune thresholds and path rules. `--explain` includes factor text, and `--include-evidence` includes evidence summaries.

## Policy

Policy controls whether a ChangePack can be verified, approved, and submitted.

### Resolution And Precedence

Effective policy is resolved field by field, highest precedence first:

1. project policy: `<root>/.draft/policy.toml`;
2. global default policy: `~/.draft/policies/default-policy.toml`;
3. Draft's built-in safe default.

A key in a higher layer overrides only that key; unspecified keys fall through. A policy file that exists but cannot be read or parsed fails closed instead of falling back to a more permissive layer.

### Canonical Policy Keys

| Key | Default | Enforced at |
| --- | --- | --- |
| `require_approval_for_submit` | `true` | submit gate |
| `block_on_critical_risk` | `true` | submit gate; also blocks without a risk report |
| `require_approval_on_high_risk` | `true` | submit gate |
| `require_reverify_on_workspace_change` | `true` | submit gate |
| `require_local_verify_for_imports` | `true` | import submit gate |
| `require_full_verify_intents` | `["security", "migration"]` | `draft verify`; escalates to `--full` |
| `require_fuzz_intents` | `["security"]` | `draft verify`; escalates to `--fuzz` |

Legacy `[submit]`, `[approval]`, and `[agent]` tables in the same policy file are still honored by legacy submit gates; canonical keys are top-level.

### Default Gates

The default v0.3.4 policy requires:

- verification and approval before submit;
- no unresolved critical risk and a canonical risk report;
- re-verification after workspace changes;
- local re-verification and approval of imported packs;
- full verification and fuzzing for security-intent packs;
- valid event, receipt, and transparency evidence for trust-relevant actions.

Draft blocks submit when a required verification or approval is missing, a blocking verification failed, the risk report is missing or critical, high risk lacks human approval, the workspace changed after verification, an import has not been locally re-verified and approved, or `.draft/` appears in the candidate. The `.draft/` block is not configurable.

Policy decisions that affect submit are visible through events and receipts, including failure reasons. Projects can tighten policy over time; policy changes should be reviewed because they alter what Draft allows to be submitted.
