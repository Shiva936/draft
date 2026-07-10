# Protocol Contracts

Draft user and operator documentation lives under `docs/`. Canonical protocol
contracts live under `proto/` so implementations, tests, and external tools can
refer to one stable source of truth.

Use `proto/specs/` for human-readable protocol rules, `proto/schemas/` for JSON
schema contracts, and `proto/test-vectors/` for compatibility fixtures.

## Specs

- [Canonicalization](../proto/specs/canonicalization.md)
- [Changepack](../proto/specs/changepack.md)
- [Close](../proto/specs/close.md)
- [Compatibility](../proto/specs/compatibility.md)
- [Composition](../proto/specs/composition.md)
- [Event Ledger](../proto/specs/event-ledger.md)
- [Future DraftHub Readiness](../proto/specs/future-drafthub-readiness.md)
- [GC](../proto/specs/gc.md)
- [Import/Export](../proto/specs/import-export.md)
- [Path Safety](../proto/specs/path-safety.md)
- [Project State](../proto/specs/project-state.md)
- [Receipt](../proto/specs/receipt.md)
- [Rollback](../proto/specs/rollback.md)
- [Save Finalization](../proto/specs/save-finalization.md)
- [Signing](../proto/specs/signing.md)
- [Stability](../proto/specs/stability.md)

## Schemas

- [Changepack Schema](../proto/schemas/changepack.schema.json)
- [Composition Schema](../proto/schemas/composition.schema.json)
- [Config Schema](../proto/schemas/config.schema.json)
- [Event Schema](../proto/schemas/event.schema.json)
- [Project State Schema](../proto/schemas/project-state.schema.json)
- [Receipt Schema](../proto/schemas/receipt.schema.json)
- [Stable Head Schema](../proto/schemas/stable-head.schema.json)
- [Verification Schema](../proto/schemas/verification.schema.json)

## Test Vectors

- [Close Clean Repo](../proto/test-vectors/close-clean-repo/vector.json)
- [Close With Pending Pack](../proto/test-vectors/close-with-pending-pack/vector.json)
- [Conflicting Packs](../proto/test-vectors/conflicting-packs/vector.json)
- [Dependent Packs](../proto/test-vectors/dependent-packs/vector.json)
- [GC Disposed Pack Cleanup](../proto/test-vectors/gc-disposed-pack-cleanup/vector.json)
- [Independent Packs](../proto/test-vectors/independent-packs/vector.json)
- [Invalid Signature](../proto/test-vectors/invalid-signature/vector.json)
- [Save Dispose Only](../proto/test-vectors/save-dispose-only/vector.json)
- [Save Merge And Dispose](../proto/test-vectors/save-merge-and-dispose/vector.json)
- [Stable Composition](../proto/test-vectors/stable-composition/vector.json)
- [Tampered Receipt](../proto/test-vectors/tampered-receipt/vector.json)
- [Unstable Composition](../proto/test-vectors/unstable-composition/vector.json)
- [Valid Pack](../proto/test-vectors/valid-pack/vector.json)
