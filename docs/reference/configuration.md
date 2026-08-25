# Configuration

Draft project configuration is private metadata stored under `.draft/`. The canonical configuration contract owns its numeric schema marker, currently `schema_version = 1`. Missing, malformed, or unsupported versions fail closed.

## Files

- `.draft/config.toml`: user display metadata, submit behavior, hooks, and verification defaults.
- `.draft/policy.toml`: submit, review, risk, and verification gates.
- `.draft/verify.toml`: local verification command configuration.
- `.draft/.ignore`: Draft-specific scan exclusions.

## CLI

```bash
draft config
draft config get user.name
draft config set user.name "Ada"
draft config set user.email "ada@example.com" --global
draft config unset user.email
```

Hook shortcuts use the same config store:

```bash
draft hook
draft hook set submit "printf %s \"{{message}}\" > .last-draft-submit"
draft hook unset submit
draft hook run <hook-name>
```

Read a hook value with `draft config get hooks.<name>`.

## User Profile

- `user.name`: optional display name.
- `user.email`: optional display/contact string.

Both values are trimmed, bounded, non-empty, and reject control data. Email is intentionally not subjected to restrictive deliverability or full RFC syntax checks because it is not an authentication identifier. Use `draft config unset user.email` to represent absence; Draft never stores an empty value.

Resolution is project `user.name`, then global `user.name`, then the built-in `unknown` fallback. The fallback is in-memory only and is never written to a configuration file. Email resolves project then global, with no built-in value.

These values are strictly non-authoritative. They do not affect actor IDs, signing or public keys, authorization, trust, receipt identity or verification, candidate attribution, workspace/source/Pack digests, event hashes, or ownership. A newly rendered presentation may include a non-authoritative display snapshot beside the stable actor ID.

Profile/config audit events record the stable actor ID, scope, changed key names, operation/correlation metadata when applicable, and resulting config digest. They do not copy profile values into immutable global/system logs.

There is no `draft identity` command and no `identity.*` compatibility alias. `.draft/identity.json`, retired XDG profile files, `[identity]`, `identity.*`, and former combined actor/profile fields are unsupported pre-release state. Normal operations fail closed without interpreting or migrating their values. `draft doctor`, relevant inspection/status paths, and `draft close` may identify the condition or safely remove an unsupported workspace, but never consume the retired profile as configuration.

## Submit Behavior

```toml
[submit]
pack_disposal = "merge_and_dispose"
message_template = "{{title}}"
```

Allowed `pack_disposal` values are:

```text
merge_and_dispose   (default) merge into Draft's stable base, then dispose
dispose_only        delegate permanence externally, then dispose
```

A missing value uses the default. An invalid value fails clearly at load time and through `draft config set`.

## Hooks

Draft supports generic, user-configured shell hooks under `hooks.*`. Draft treats every hook as opaque: it does not infer whether a command commits to Git, updates Jujutsu, runs a script, pushes to a forge, or performs another external action.

### Configuration Shapes

A raw submit hook is one before-submit command:

```toml
[hooks.submit]
kind = "raw"
command = "printf %s \"{{message}}\" > .last-draft-submit"
```

A rich entry adds execution controls:

```toml
[hooks.submit]
kind = "entry"

[hooks.submit.entry]
command = "./scripts/before-draft-submit.sh \"{{message}}\""
enabled = true
shell = "default"
cwd = "workspace"
timeout_ms = 300000
continue_on_error = false

[hooks.submit.entry.env]
CI = "1"
```

The phased form supports multiple before and after commands:

```toml
[hooks.submit]
kind = "phases"
before = [{ kind = "raw", command = "cargo fmt --check" }]
after  = [{ kind = "raw", command = "git add -A && git commit -m \"{{message}}\"" }]
```

Before hooks run before final project-state verification and finalization. After hooks run after `stable_head` advancement in `merge_and_dispose` mode but before pack disposal. A required non-zero exit fails submit and preserves the pack; `continue_on_error = true` allows finalization to continue and records a submitted-with-hook-failure result.

`hooks.verify` uses the same raw or rich entry model for verification commands. Future command-specific hooks use the same `hooks.<command>` namespace; hook names do not introduce native VCS, publication, remote, or marketplace concepts.

### Placeholders

Hook commands use `{{name}}` placeholders. Single-brace placeholders are invalid.

Built-in placeholders are:

```text
{{message}}
{{title}}
{{description}}
{{task_id}}
{{execution_id}}
{{pack_id}}
{{receipt_id}}
{{actor_name}}
{{timestamp}}
{{verified}}
{{risk_level}}
{{files_changed}}
{{workspace_root}}
{{hook_name}}
{{hook_phase}}
```

For the stable placeholder name, `actor_name` carries the stable security actor ID. It does not resolve `user.name`; user profile values never enter hook commands or their environment.

Missing placeholders fail before execution and obey `continue_on_error`.

### Dynamic Variables

Hook-capable commands accept `--var` as a tail marker:

```bash
draft submit auth-refactor --var ticket="AUTH-123" release="v0.3.4"
```

Every token after `--var` must be `key=value`; Draft flags are not allowed after the marker. Variable names must match `[a-zA-Z_][a-zA-Z0-9_]*` and cannot override built-ins.

Dynamic values are available as placeholders and environment variables:

```text
{{ticket}}
{{release}}
DRAFT_VAR_TICKET=AUTH-123
DRAFT_VAR_RELEASE=v0.3.4
```

### Environment, Results, And Receipts

Draft exports built-ins using names such as `DRAFT_HOOK_NAME`, `DRAFT_HOOK_PHASE`, `DRAFT_WORKSPACE_ROOT`, and `DRAFT_RECEIPT_ID`. Dynamic variables use `DRAFT_VAR_<UPPERCASE_NAME>`. Values from `[hooks.<name>.env]` are added without removing or renaming Draft-provided metadata.

Draft interpolates placeholders before execution and records the command hash, stdout, stderr, exit code, timestamps, executor, working directory, and receipt references. Submit receipts distinguish:

```text
native_submit_status = "submitted" | "failed"
hook_status          = "not_configured" | "skipped" | "succeeded" | "failed"
overall_status       = "submitted" | "failed" | "submitted_with_hook_failure"
```

Example commands include:

```toml
[hooks.submit]
kind = "phases"
before = [
  { kind = "raw", command = "./scripts/check-before-submit.sh" },
]
after = [
  { kind = "entry", entry = { command = "./scripts/publish-after-review.sh \"{{message}}\" \"{{ticket}}\"", timeout_ms = 120000 } },
]
```

This remains user-scripted hook execution. Draft v0.3.4 has no native push, forge, pull-request, hosted-review, marketplace, cloud-sync, or GitHub feature.

Hooks are not sandboxed. Configure them as carefully as any local shell command. Draft never runs submit hooks when `.draft/` appears in the submit candidate.

## Verification Configuration

`verification.default_profile` selects the default local verification profile. Profile commands are read from Draft verification configuration. See [Review, Verification, And Policy](review-and-policy.md#verification).

## Policy Configuration

Policy values live in `.draft/policy.toml`. Defaults require verification and approval before submit. The `.draft/` submit-candidate block is not configurable. See [Review, Verification, And Policy](review-and-policy.md#policy).

## Resolution And Precedence

Draft loads supported user-level configuration first, then overlays workspace configuration. Resolution is field-based, so workspace values are authoritative for the keys they define. Policy has its own explicit precedence rules.

## Canonical Config Hash

Parsed configuration contributes a formatting- and comment-insensitive `config_hash` to the deterministic verification cache key:

```text
verification_key = H(workspace_hash + config_hash + toolchain_hash + verification_command_hash + environment_hash)
```

Changing configuration deterministically invalidates prior verification results.

## Ignore Rules

Draft uses `.draft/.ignore` for Draft-specific scan exclusions. Draft does not import ignore rules from other tools; it scans the workspace directly and applies only Draft rules plus the hard `.draft/` exclusion.

```bash
draft ignore add "notes/"
draft ignore remove "notes/"
draft ignore list
```

Rules are stored as plain lines. Blank lines and comments are ignored. Draft supports path-prefix and file-pattern matching, and negated rules can re-include a path unless it is below `.draft/`, which can never be re-included. Use forward slashes; Draft normalizes platform path separators before matching.

Keep ignore rules narrow. Broad rules can hide files from Packs, verification, and rollback planning. When in doubt, leave files visible for review or policy to decide.
