# Review, Verification, And Policy

Draft separates evidence collection, risk assessment, review and the final Decision, so a person can inspect exactly why a revision may or may not be promoted.

## Review And Approval

### Review

```bash
draft change review <rev-id>
draft change review <rev-id> --comment "looks good"
```

Review records that the Change entered the human review boundary. Inspect:

- the resources that changed, and what could not be determined about them;
- verification results in all five states, with the producer and the decision that permitted each check;
- risk findings and policy blockers, or the fact that risk could not be assessed;
- receipts and prior decisions.

Review units are resource-level by default. Where an installed comparison capability derived finer units — a region of a contributed coordinate space, a key of a contributed key space — decisions attach to those instead, which is how fine-grained review survives without Draft knowing what a line is. Installing `draft.text.document` is what turns a text file's changes into line-level review units, in the `draft.text.document/line` coordinate space.

### Approval And Rejection

```bash
draft change decide --approve -p <Change> --reason "verified locally"
draft change decide --reject -p <Change> --reason "needs changes"
```

Default policy requires an approving Decision before a promotion, and high-risk changes require human approval when that policy is enabled. A Decision is local Draft metadata, not a hosted code-review approval.

## Draft Console

`draft console tui --project <workspace-id|path>` opens the terminal form of Draft Console. It is designed for repeated review work where a user needs to inspect status, evidence, risk, policy, approvals, receipts, and rollback options without leaving Draft.

The first view is summary-first: overview, hotspots, evidence gaps, provenance, readiness, and available actions. Element-level impact and risk evidence appear before any resource-by-resource detail.

Console sections cover:

- workspace status and latest scan time;
- Change list, selection, file changes, and overlap indicators;
- verification and gate-readiness counts;
- risk findings, evidence gaps, and policy blockers;
- decisions and approve or reject actions;
- compare and compose actions;
- gate readiness, receipts, and recovery;
- service connection state.

The TUI receives authoritative models and short-lived action capabilities from `draftd`; it never reads a workspace or computes action eligibility itself. It preserves loaded data as stale if the daemon disconnects and disables mutations until a new session and fresh descriptors are available.

## Verification

Verification runs checks and records their results as Draft evidence.

```bash
draft change evidence run -p <Change-id-or-name>
```

Draft resolves the checks that apply to the change, runs each one through its single authorized execution boundary, and captures stdout, stderr, exit code, and timing in a verification receipt. Every check names the extension that contributed it, the artifact attestation it was accepted under, and the authorization decision that permitted the run.

A check ends in one of five states, and they are deliberately distinct:

| State            | Meaning                                                   |
| ---------------- | --------------------------------------------------------- |
| `passed`         | the check ran and was satisfied                           |
| `failed`         | the check ran and was not satisfied                       |
| `unavailable`    | no installed extension contributes a check for this       |
| `not_evaluated`  | a check applies but has not been run yet                  |
| `not_applicable` | a check exists but nothing in this change is in its scope |

`unavailable` and `not_applicable` are not interchangeable: `not_applicable` means Draft asked and nothing was in scope, while `unavailable` means there was nothing to ask. With no verification capability installed the state is `unavailable`, because that is the one an installation would change.

The aggregate follows a fixed lattice: any required `failed` makes the whole `failed`; otherwise any required `unavailable` makes it `unavailable`; otherwise any required `not_evaluated` makes it `not_evaluated`; otherwise, if any required check passed, `passed`; otherwise `not_applicable`. **An empty check set can never aggregate to `passed`** — a project with no verification capability installed reports `unavailable` with install guidance, not success.

When verification fails:

1. Inspect the receipt and its stdout and stderr objects.
2. Fix the workspace.
3. Create or update the Change.
4. Run verification again.

## Risk

`draft change assess` is deterministic and local. Its result is one of two things, and they are not interchangeable:

- `unassessed` — no rule was available to judge this change. The report names why and what would supply one. This is **not** a low score; Draft has no opinion, and says so.
- `assessed` — rules ran. The report carries a score, band, stable reason codes, per-rule results with the extension that contributed each rule, hotspots, evidence gaps, and a receipt.

Draft ships no risk rules of its own. "Authentication", "payments" and "migrations" are software-project vocabulary; a recording session or a CAD assembly has entirely different sensitivities. Rules arrive from an installed extension's `risk_rule` contribution or from the project's own `.draft/risk.toml`, and both are evaluated identically.

Thresholds follow ordinary precedence: `.draft/risk.toml` wins where it states them; otherwise installed `control_policy` presets compose conservatively — the tightest band any of them asks for — over Draft's defaults.

`--explain` includes factor text, and `--include-evidence` includes evidence summaries.

## Protections

A protection is something a change is never allowed to touch. Draft owns exactly one, and it is structural: `.draft/**`, its own control plane, which no configuration and no extension can unprotect.

Everything else is domain judgement. Credential files, key material and registry tokens are protected by an installed `control_policy` — `draft.filesystem.policy` covers `.env`, `*.pem`, `*.key`, `id_rsa`, `*.p12`, `*.pfx` and `.aws/credentials`; `draft.software.project` adds `.npmrc` and `.pypirc` — or by the project's own `config.toml`. Protections compose as a union: no layer can remove one another layer added, and a refusal names which source asked for it.

A protection written for `file`-scheme locators never matches another scheme, so a rule about `*.key` cannot silently capture a catalog resource whose identifier happens to contain a dot.

## Policy

Policy controls whether a revision can be verified, approved, and promoted.

### Resolution And Precedence

Effective policy is resolved field by field, highest precedence first:

1. project policy: `<root>/.draft/policy.toml`;
2. global default policy: `~/.draft/policies/default-policy.toml`;
3. Draft's built-in safe default.

A key in a higher layer overrides only that key; unspecified keys fall through. A policy file that exists but cannot be read or parsed fails closed instead of falling back to a more permissive layer.

### Canonical Policy Keys

| Key                                    | Default | Enforced at                                        |
| -------------------------------------- | ------- | -------------------------------------------------- |
| `require_approval_for_promotion`       | `true`  | promotion gate                                     |
| `block_on_critical_risk`               | `true`  | promotion gate; also blocks without a risk report  |
| `require_approval_on_high_risk`        | `true`  | promotion gate                                     |
| `require_reverify_on_workspace_change` | `true`  | promotion gate                                     |
| `require_local_verify_for_imports`     | `true`  | import promotion gate                              |
| `require_full_verify_intents`          | `[]`    | `draft change evidence run`; escalates to `--full` |
| `require_fuzz_intents`                 | `[]`    | `draft change evidence run`; escalates to `--fuzz` |

The two intent keys are empty by default and deliberately so. An intent is a namespaced identifier from a contributed vocabulary, so an intent id Draft invented would name a vocabulary nothing declares and could never match. Escalations arrive from the same package that declares the intent — `draft.software.project` escalates its own `security` intent to full and exploratory verification, and its `migration` intent to full — or from the project's `policy.toml`. Contributed escalations only ever add, so neither a second extension nor a permissive file can relax what another already requires.

Only the canonical top-level policy keys are accepted. Unknown or malformed policy shapes fail closed and are never reinterpreted through another reader.

### Default Gates

The default policy requires:

- verification and an approving Decision before a promotion;
- no unresolved critical risk and a canonical risk report;
- re-verification after workspace changes;
- local re-verification and approval of imported Changes;
- full verification and fuzzing for security-intent Changes;
- valid event, receipt, and transparency evidence for trust-relevant actions.

Draft refuses a promotion when a required verification or approving Decision is missing, a blocking verification failed, the risk report is missing or critical, high risk lacks human approval, the workspace changed after verification, an import has not been locally re-verified and approved, or `.draft/` appears in the candidate. The `.draft/` block is not configurable.

Promotion also fails closed on unresolved observation or derivation gaps: if Draft could not see part of the project, it will not treat what it could not observe as unchanged. A waiver bound to the evaluation is the explicit way past that, and the receipt records that the change was accepted with waived uncertainty rather than with complete observation.

Not everything can be waived. A protection exists precisely to be what nobody can wave through in a hurry. Review and approval cannot be waived either, for a different reason: a waiver _is_ a human judgement, so accepting one in their place would mean somebody signing off on not having to sign off. Everything else — verification, risk, observation and derivation completeness, reviewability, recovery readiness — can be waived explicitly, by name, with a reason and an expiry, and the requirement then reads as satisfied _despite_ not being met rather than quietly disappearing.

A submission binds three acceptance identities: the requirements in force, the judgement made against them, and the artifacts those requirements were assembled from. The first is semantics — two package revisions whose policy means the same thing share it, which is what keeps an existing approval valid across a routine upgrade. The third is assembly, recorded separately so a receipt can still name the exact artifact it relied on.

Policy decisions that affect promotion are visible through Activity and receipts, including refusal reasons. Projects can tighten policy over time; policy changes should be reviewed because they alter what Draft allows to be accepted.
