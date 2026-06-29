# v0.4 Reserved Target Notes

`target.remote` is reserved for later design work.

## v0.3.0 Behavior

- `target.remote` config writes are rejected.
- No network target execution exists.
- Save receipts contain local-only result fields.
- The only executable target key is `target.local`.

## Compatibility Goal

The save receipt schema keeps external result data grouped so later schema versions can add new result kinds without rewriting v0.3.0 receipts.

## Non-Goals

This document does not specify network behavior, credentials, hosted review, publishing, or synchronization. Those topics require a separate design and threat model.
