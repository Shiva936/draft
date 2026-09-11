# Configuration

Draft project configuration is private metadata stored under `.draft/`. The canonical configuration contract owns its numeric schema marker, currently `schema_version = 1`. Missing, malformed, or unsupported versions fail closed.

## Files

- `.draft/config.toml`: user display metadata, hooks, and verification defaults.
- `.draft/policy.toml`: gate, review, risk, and verification policy.
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
draft config hook
draft config hook set verify "cargo test"
draft config hook unset verify
draft config hook run <hook-name>
```

Read a hook value with `draft config get hooks.<name>`.

## User Profile

- `user.name`: optional display name.
- `user.email`: optional display/contact string.

Both values are trimmed, bounded, non-empty, and reject control data. Email is intentionally not subjected to restrictive deliverability or full RFC syntax checks because it is not an authentication identifier. Use `draft config unset user.email` to represent absence; Draft never stores an empty value.

Resolution is project `user.name`, then global `user.name`, then the built-in `unknown` fallback. The fallback is in-memory only and is never written to a configuration file. Email resolves project then global, with no built-in value.

These values are strictly non-authoritative. They do not affect actor IDs, signing or public keys, authorization, trust, receipt identity or verification, candidate attribution, workspace/source/Change digests, event hashes, or ownership. A newly rendered presentation may include a non-authoritative display snapshot beside the stable actor ID.

Profile/config audit events record the stable actor ID, scope, changed key names, operation/correlation metadata when applicable, and resulting config digest. They do not copy profile values into immutable global/system logs.

There is no `draft identity` command and no `identity.*` compatibility alias. `.draft/identity.json`, retired XDG profile files, `[identity]`, `identity.*`, and former combined actor/profile fields are unsupported pre-release state. Normal operations fail closed without interpreting or migrating their values. `draft doctor`, relevant inspection/status paths, and `draft maintenance remove-project` may identify the condition or safely remove an unsupported workspace, but never consume the retired profile as configuration.

## Hooks

Draft supports generic, user-configured shell hooks under `hooks.*`. Draft treats every hook as opaque: it does not infer whether a command commits to Git, updates Jujutsu, runs a script, pushes to a forge, or performs another external action.

### Configuration Shapes

A raw hook is one command:

```toml
[hooks.verify]
kind = "raw"
command = "cargo test"
```

A rich entry adds execution controls:

```toml
[hooks.verify]
kind = "entry"

[hooks.verify.entry]
command = "./scripts/verify.sh"
enabled = true
shell = "default"
cwd = "workspace"
timeout_ms = 300000
continue_on_error = false

[hooks.verify.entry.env]
CI = "1"
```

A required non-zero exit fails the hook and is reported as a `HOOK_FAILED` refusal; `continue_on_error = true` records the failure and continues.

A hook is never a promotion. Nothing a hook does changes what the project accepts: that is Promotion's sole authority, and it happens only on an approving Decision citing a satisfied Gate. Future command-specific hooks use the same `hooks.<command>` namespace; hook names introduce no native VCS, publication, remote, or marketplace concepts.

### Placeholders

Hook commands use `{{name}}` placeholders. Single-brace placeholders are invalid.

Built-in placeholders are:

```text
{{message}}
{{title}}
{{description}}
{{task_id}}
{{execution_id}}
{{change_id}}
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
draft config hook run verify --var ticket="AUTH-123" release="v0.3.4"
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

Draft interpolates placeholders before execution and records the command hash, stdout, stderr, exit code, timestamps, executor and working directory as an `OperationExecuted` Activity event. A hook result reports its own outcome:

```text
exit_code    the process's exit status
stdout_ref   the object holding captured output
stderr_ref   the object holding captured error output
```

An example:

```toml
[hooks.verify]
kind = "entry"

[hooks.verify.entry]
command = "./scripts/check.sh \"{{ticket}}\""
timeout_ms = 120000
```

This remains user-scripted hook execution. Draft v0.3.4 has no native push, forge, pull-request, hosted-review, marketplace, cloud-sync, or GitHub feature.

Hooks are not sandboxed. Configure them as carefully as any local shell command. Draft never runs a hook when `.draft/` appears in the change candidate.

## Verification Configuration

`verification.default_profile` selects the default local verification profile. Profile commands are read from Draft verification configuration. See [Review, Verification, And Policy](review-and-policy.md#verification).

## Policy Configuration

Policy values live in `.draft/policy.toml`. Defaults require verification and an approving Decision before a promotion. The `.draft/` change-candidate block is not configurable. See [Review, Verification, And Policy](review-and-policy.md#policy).

## Protections

`[protected].protected_resources` in `.draft/config.toml` lists the resources this project refuses to let a change touch. Each rule carries a predicate and a reason, and a refusal names the rule that caused it.

Protections compose as a union across four sources — Draft itself, the user's global configuration, this project's configuration, and any installed `control_policy` — and no layer can relax one another layer added. Draft's own contribution is exactly one rule, `.draft/**`, applied structurally ahead of every list: the control plane is unreachable regardless of what any configuration or contribution says.

The name is deliberate. A protection applies to a _resource_, identified by an opaque locator, and only the `file` scheme's bodies happen to be paths — so a rule written as a path glob matches nothing under another scheme rather than matching the wrong thing. A refused write reports `PROTECTED_RESOURCE_ACCESS`.

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
draft config ignore add "notes/"
draft config ignore remove "notes/"
draft config ignore list
```

Rules are stored as plain lines. Blank lines and comments are ignored. Draft supports path-prefix and file-pattern matching, and negated rules can re-include a path unless it is below `.draft/`, which can never be re-included. Use forward slashes; Draft normalizes platform path separators before matching.

Keep ignore rules narrow. Broad rules can hide files from Changes, verification, and rollback planning. When in doubt, leave files visible for review or policy to decide.

## Extension state

Extension state lives in the global Draft home, not in a project:

- `extensions/registry.json` — installed packages, their content hashes and enabled state.
- `extensions/authorizations.json` — capability grants, each bound to one exact artifact, plus the superseded grants kept for audit.
- `extensions/sources.json` — configured catalog sources, including whether each is enabled and whether it is built into this Draft build.
- `extensions/packages/<id>/` — the installed package contents.
- `extensions/removed/` — packages retained after uninstall or replacement.

None of these are edited by hand. Grants in particular are created only by `draft extension authorize` (or `--grant` on install and update) and are invalidated automatically whenever the artifact they name changes.

A project's own `verify.toml` is unaffected by extensions: an installed package can neither add to nor remove from the checks a project configured for itself. Contributed checks are added alongside, keyed by their own namespaced ids, and the two sets aggregate through the same five-state lattice.

The same is true of `risk.toml`. A rule a project writes for itself is evaluated by exactly the same path as one an extension contributes, so expressing a domain judgement never requires publishing a package. Where a contributed rule reuses a code the project already configured, the project's rule wins and the contributed one is dropped — configuration is the project's own voice.
