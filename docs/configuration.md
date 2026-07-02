# Configuration

Draft configuration is local project metadata stored under `.draft/`.

## Files

- `.draft/config.toml`: identity, save message, hooks, and verification defaults.
- `.draft/policy.toml`: save and review gates.
- `.draft/verify.toml`: local verification command configuration.
- `.draft/.ignore`: Draft-specific ignore rules.

## CLI

```bash
draft config
draft config -k identity.username
draft config set identity.username "Ada"
draft config unset identity.email
```

Hook shortcuts use the same config store:

```bash
draft hook
draft hook set save "printf %s \"{{message}}\" > .last-draft-save"
draft hook unset save
```

## Common Keys

- `identity.username`: human-readable actor name.
- `identity.email`: optional actor email metadata.
- `save.message_template`: template used to render save messages.
- `hooks.save`: optional opaque command run after save approval and safety checks.
- `verification.default_profile`: default local verification profile.

## Policy

Default policy requires verification and approval before save. `.draft/` save-candidate blocking is not configurable.

See [hooks.md](hooks.md), [verification.md](verification.md), [safety-model.md](safety-model.md), and [config-rules.md](config-rules.md).

