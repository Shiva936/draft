# Official Draft extensions

The first-party extension packages, the tool that publishes them, and the conformance tests that keep them honest.

This directory is a **separate Cargo workspace**. The Draft platform never builds it, and the root manifest excludes it, so deleting this directory is a genuine no-op for Draft. That is deliberate: these sources are shaped like the standalone `draft-extensions` repository they will one day become, and moving them there should be a build, release and catalog change rather than an implementation change.

```
packages/   nine declarative packages — data only, no code
tools/      the packaging and signing tool
tests/      conformance tests that need no Draft platform
scripts/    the standalone gate CI runs
```

## What a package is

A package is data. It declares contributions — resource adapters, resource classification, comparison, element extraction, presentation bindings, tool actions, verification checks, risk rules, policy presets, intent vocabularies, task templates, candidate presets and documentation — and Draft interprets them. No package contains an entrypoint, a script, native code, or anything Draft loads and runs on the package's behalf.

Every identifier a package mints lives in a namespace the package owns, and packaging refuses one that does not: a package claiming another publisher's class, check or intent id would have its rules silently apply to their work. Payloads are decoded and validated at packaging time too, so a malformed contribution fails to package rather than installing cleanly and contributing nothing.

The most powerful thing a package can declare is a _structured command_: a program name, an argument vector, a workspace-relative working directory and a time limit. Draft runs it itself, spawning the program directly with its arguments, under a cleared environment and an enforced time limit. There is no shell in that path, so quoting, globbing, redirection and command chaining are not available to a package.

A package that declares a command must request the `process.execute` permission, and that permission does nothing until a user authorizes it for the exact installed artifact. Installing grants nothing.

## The only Draft dependency

Everything here builds against `draft-extension-contract` and nothing else. That crate is the public boundary: the package format, the signed catalog format, canonical JSON, and signature verification. It does not depend on Draft's core, services, daemon or stores.

`extensions/scripts/check-standalone.sh` enforces this, and CI runs it. If a change here needs something from Draft's private source, that is a signal the boundary is in the wrong place — not a reason to reach across it.

## Publishing

```
draft-extension-packager validate packages
draft-extension-packager build packages <out> --catalog-id draft-official
draft-extension-packager sign <out> --key <file> --key-id <id>
draft-extension-packager verify <out>
```

`validate`, `build` and `verify` need no key material at all. Only `sign` does, and it reads a key the caller supplies.

**The tool never generates, stores or commits a signing key.** Production keys belong to an authorized signing environment; ordinary package generation cannot create a root of trust by accident. CI supplies an ephemeral key the same way, and deletes it with its scratch directory.

Catalog search metadata — display name, description, keywords and contributed capabilities — is _derived_ from each validated `extension.json`, never authored a second time. A catalog therefore cannot describe a package differently from how the package describes itself, and the conformance gate checks exactly that.

## Adding a package

1. Create `packages/<id>/` with an `extension.json`, its `contributions/`, `docs/readme.md` and `LICENSE.txt`.
2. Give it a description and keywords: they are what makes it findable, since catalog metadata is derived from the manifest.
3. Request `process.execute` if — and only if — it declares a command. The conformance tests check both directions of that.
4. Run `cargo test` here, then `scripts/check-standalone.sh`.

Add only what Draft can already act on. A contribution kind Draft does not interpret is not a feature; it is an unused field that will drift.
