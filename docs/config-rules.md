# Draft Configuration Rules

Draft configuration lives in `.draft/config.toml`. The CLI supports `draft config set`, `get`, `unset`, and `list`.

## Identity

`identity.username`

Human-readable actor name used in events, receipts, reviews, and approvals.

`identity.email`

Optional actor email used only as Draft metadata.

## Hooks

`hooks.save`

An optional opaque command hook. It may be a raw command string or a rich hook entry. Draft runs it after a changepack is verified, approved, policy-allowed, and confirmed to have no `.draft/` save-candidate violation.

`hooks.verify`

Configuration namespace for verification hooks. It uses the same hook entry model as `hooks.save`.

`save.message_template`

Template used to render the save message. The rendered message is stored as an object and can be interpolated into `hooks.save` with `{{message}}`.

Hook command placeholders use `{{name}}`. Hook-capable commands may supply dynamic variables through `--var key=value`; these become `{{key}}` placeholders and `DRAFT_VAR_KEY` environment variables.

## Verification

`verification.default_profile`

Name of the default verification profile. The profile commands are read from Draft verification configuration.

## Policy

Policy values are stored in `.draft/policy.toml`. Defaults require verification and approval before save. See [policy.md](policy.md).

## Precedence

Draft loads workspace config first, then supported user-level config where available. Workspace config should be considered authoritative for project behavior.
