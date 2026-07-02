# Hooks

Draft v0.3.1 supports generic command hooks under `hooks.*`.

Hooks are user-configured shell commands that Draft runs after or around Draft commands. Draft treats every hook as opaque. It does not infer whether a hook commits to Git, updates Jujutsu, runs a script, pushes to GitHub, or performs any other external action.

## Configuration

Raw string form:

```toml
[hooks]
save = "git add . && git commit -m \"{{message}}\""
```

Rich entry form:

```toml
[hooks.save]
command = "git add . && git commit -m \"{{message}}\""
enabled = true
phase = "after_success"
shell = "default"
cwd = "workspace"
timeout_ms = 300000
continue_on_error = false

[hooks.save.env]
CI = "1"
```

Raw string hooks are equivalent to enabled rich hooks with `phase = "after_success"`, `shell = "default"`, `cwd = "workspace"`, no extra env, and `continue_on_error = false`.

## Hook Names

`hooks.save` runs only after `draft save` succeeds its approval, verification, policy, and `.draft/` safety checks.

`hooks.verify` uses the same hook configuration model for verification-related command hooks.

Future command-specific hooks use the same `hooks.<command>` shape. Draft must not introduce external-action slots, integration adapters, commits, publication, remotes, push, PRs, or VCS-native concepts through hook names.

## Placeholders

Hook commands use canonical `{{name}}` placeholders. Legacy single-brace placeholders are not valid hook syntax.

Built-in placeholders:

```text
{{message}}
{{title}}
{{description}}
{{task_id}}
{{run_id}}
{{changepack_id}}
{{receipt_id}}
{{actor_name}}
{{actor_email}}
{{timestamp}}
{{verified}}
{{risk_level}}
{{files_changed}}
{{workspace_root}}
{{hook_name}}
{{hook_phase}}
```

Missing placeholders fail the hook before execution and obey `continue_on_error`.

## Dynamic Variables

Hook-capable commands accept `--var` as a tail marker:

```bash
draft save auth-refactor --var ticket="AUTH-123" release="v0.3.1"
```

Every token after `--var` must be `key=value`. Normal Draft flags are not allowed after `--var`. Variable names must match `[a-zA-Z_][a-zA-Z0-9_]*` and must not override built-in placeholders.

Dynamic variables are available as placeholders and environment variables:

```text
{{ticket}}
{{release}}
DRAFT_VAR_TICKET=AUTH-123
DRAFT_VAR_RELEASE=v0.3.1
```

## Environment

Draft exports built-ins and dynamic variables into the hook environment. Built-ins use `DRAFT_` names such as `DRAFT_HOOK_NAME`, `DRAFT_HOOK_PHASE`, `DRAFT_WORKSPACE_ROOT`, and `DRAFT_RECEIPT_ID`. Dynamic variables use `DRAFT_VAR_<UPPERCASE_NAME>`.

Values from `[hooks.<name>.env]` are added after Draft built-ins. They must not remove or rename Draft-provided hook metadata.

## Results And Receipts

Draft interpolates placeholders before execution, records the command hash, captures stdout, stderr, exit code, start/end timestamps, executor, and working directory, and attaches hook results to the receipt keyed by hook name.

For `hooks.save`, the receipt records:

```text
native_save_status = "saved"
hook_status = "succeeded" | "failed" | "skipped"
overall_status = "saved" | "failed" | "saved_with_hook_failure"
```

If a required hook exits non-zero and `continue_on_error = false`, `overall_status = "failed"`. If `continue_on_error = true`, `overall_status = "saved_with_hook_failure"`.

## Examples

Commit to Git:

```toml
[hooks.save]
command = "git add . && git commit -m \"{{message}}\""
```

Update another local VCS:

```toml
[hooks.save]
command = "jj describe -m \"{{message}}\""
```

Run a local script:

```toml
[hooks.save]
command = "./scripts/after-draft-save.sh \"{{message}}\" \"{{ticket}}\""
timeout_ms = 120000
```

Trigger a user-owned remote command:

```toml
[hooks.save]
command = "git add . && git commit -m \"{{message}}\" && git push origin main"
```

That behavior is only user-scripted hook execution. Draft v0.3.1 has no native push, forge, PR, integration adapter, or GitHub feature.

## Safety

Draft never runs `hooks.save` if `.draft/` appears in the save candidate. Draft records the failed save, emits `save.completed` with failure status, and skips hook execution.

Hooks are not sandboxed. Configure them as carefully as any local shell command.
