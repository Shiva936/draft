# Contributing To Draft

Draft is a local-first change-control tool. Contributions should preserve the existing boundary: Draft owns `.draft/`, verifies and saves changepacks locally, and treats `hooks.*` as opaque command strings.

## Development Setup

Install a stable Rust toolchain, then run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The workspace is split into:

- `core/`: Draft-native data model and local store behavior;
- `cli/`: command-line interface that works without a daemon;
- `tui/`: review cockpit rendering and interaction layer;
- `services/`: optional local background services;
- `docs/`: public user and maintainer documentation.

## Design Rules

- Keep `.draft/` hard-excluded from user change candidates.
- Keep CLI flows functional without `draftd`.
- Prefer deterministic serialized data for anything hashed.
- Record important actions as receipts and events.
- Add tests for every behavior that affects save, rollback, policy, evidence, or event integrity.
- Do not add hidden network behavior.
- Do not infer external system semantics from local files or command strings.

## Pull Request Expectations

Every functional change should include focused tests, documentation updates when user-visible behavior changes, a release-compliance note when readiness changes, and passing formatting, linting, and tests.

## Documentation Style

Docs should be practical and precise. Show command sequences, explain stored artifacts, and call out failure modes. Avoid promising behavior that is not covered by tests.

## Reporting Security Issues

Use [SECURITY.md](SECURITY.md) for supported reporting channels and disclosure expectations.
