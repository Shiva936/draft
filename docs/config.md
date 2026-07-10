# Config

Draft's project configuration lives in `.draft/config.toml`. This page covers
the v0.3.3 save/hook settings; [Configuration](configuration.md) documents the
full key reference and [Configuration Rules](config-rules.md) the resolution
order.

## Save Behavior

```toml
[save]
pack_disposal = "merge_and_dispose"
message_template = "{{title}}"
```

Allowed `pack_disposal` values:

```text
merge_and_dispose   (default) merge into Draft's stable base, then dispose
dispose_only        delegate permanence externally, then dispose
```

A missing value uses the default. An invalid value fails clearly at load time
and via `draft config set`.

## Save Hooks

```toml
[hooks.save]
before = [{ command = "cargo fmt --check" }]
after  = [{ command = "git add -A && git commit -m {{message}}" }]
```

Before hooks run before finalization; after hooks run after `stable_head`
advancement but before disposal. A non-zero exit fails the save and preserves
the pack (per-hook `continue_on_error = true` opts out). See [Hooks](hooks.md)
for the full entry shape, template variables, and sandboxing rules.

## Canonical Config Hash

The parsed config (formatting- and comment-insensitive) contributes a
`config_hash` to the deterministic verification cache key:

```text
verification_key = H(workspace_hash + config_hash + toolchain_hash
                     + verification_command_hash + environment_hash)
```

Changing config deterministically invalidates prior verification results.
