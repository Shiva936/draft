# Contributing To Draft

Draft is a local-first change-control tool. Contributions should preserve the existing boundary: Draft owns `.draft/`, verifies and submits packs locally, and treats `hooks.*` as opaque command strings.

## Development Setup

Install a stable Rust toolchain, then run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo audit --deny warnings

cd console/web
npm ci
npm run typecheck
npm audit --audit-level=high
npm run build
```

The workspace is split into:

- `core/`: Draft-native data model and local store behavior;
- `cli/`: command-line interface that works without a daemon;
- `console/application/`: typed Rust client for the Console application protocol;
- `console/tui/`: reducer-driven terminal frontend with no direct core or project-filesystem access;
- `console/`: browser Console gateway, `web/` source, and embedded `dist/` assets;
- `services/`: optional local background services;
- `docs/`: public user and maintainer documentation.

## Design Rules

- Keep `.draft/` hard-excluded from user change candidates.
- Keep CLI flows functional without `draftd`.
- Prefer deterministic serialized data for anything hashed.
- Record important actions as receipts and events.
- Add tests for every behavior that affects submit, rollback, policy, evidence, or event integrity.
- Do not add hidden network behavior.
- Do not infer external system semantics from local files or command strings.

## Pull Request Expectations

Every functional change should include focused tests, documentation updates when user-visible behavior changes, an update to the [release-compliance matrix](docs/release-compliance.md) when readiness changes, and passing formatting, linting, tests, and security audits.

## Documentation Style

Docs should be practical and precise. Show command sequences, explain stored artifacts, and call out failure modes. Avoid promising behavior that is not covered by tests.

## Reporting Security Issues

Use [SECURITY.md](SECURITY.md) for supported reporting channels and disclosure expectations.
