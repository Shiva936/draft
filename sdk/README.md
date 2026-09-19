# Draft SDK

The `sdk/` directory contains Draft's public, dependency-light libraries. They are designed to be consumed by extension authors, publishers, and other tools without linking Draft's runtime, services, repository state, or Console.

Being under `sdk/` means externally consumable and dependency-light; it does not by itself promise permanent semver/API stability while Draft remains pre-release.

Current libraries:

- [`extension-contract`](extension-contract/README.md): portable extension manifests, contributions, package and catalog rules, canonicalization, compatibility, digests, and Ed25519 verification.
