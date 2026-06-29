# Draft Configuration Rules

Draft configuration lives in `.draft/config.toml`. The CLI supports `draft config set`, `get`, `unset`, and `list`.

## Identity

`identity.username`

Human-readable actor name used in events, receipts, reviews, and approvals.

`identity.email`

Optional actor email used only as Draft metadata.

## Local Save Target

`target.local`

An optional opaque command string. Draft runs it after a changepack is verified, approved, policy-allowed, and confirmed to have no `.draft/` save-candidate violation.

`target.message_template`

Template used to render the save message. The rendered message is stored as an object and can be interpolated into `target.local` with `{message}`.

## Verification

`verification.default_profile`

Name of the default verification profile. The profile commands are read from Draft verification configuration.

## Policy

Policy values are stored in `.draft/policy.toml`. Defaults require verification and approval before save. See [policy.md](policy.md).

## Reserved Keys

`target.remote` is reserved for a later version and is rejected in v0.3.0. Reserved keys should not be used by scripts because their behavior is intentionally undefined in this release.

## Precedence

Draft loads workspace config first, then supported user-level config where available. Workspace config should be considered authoritative for project behavior.
