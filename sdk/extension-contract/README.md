# draft-extension-contract

`draft-extension-contract` is the dependency-light Rust implementation of Draft's public, portable extension contract. Extension authors, publishers, catalog tooling, and Draft itself use the same types and rules without pulling in Draft Core or any runtime service.

It defines extension manifests and contribution vocabulary, package layout and size rules, Draft API compatibility metadata, canonical JSON and package digests, signed catalog documents, and Ed25519 verification. It does not install or download extensions, decide trust or authorization, execute declared commands, manage lifecycle state, read Draft repositories, or implement daemon and Console behavior.

```toml
[dependencies]
draft-extension-contract = "0.3.4"
```

Inside the Draft monorepo, downstream extension tooling pairs that version with `path = "../sdk/extension-contract"` as a local-development override. Moving that tooling to another repository changes only the dependency source.

Draft is pre-release software. This crate is intended for external consumption, but its Rust API does not yet carry a permanent semver-stability promise. The packaged v1 compatibility vectors freeze the portable wire behavior implemented by format revision 1.

Licensed under Apache-2.0.
