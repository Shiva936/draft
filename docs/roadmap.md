# Roadmap

## Shipped in v0.2.0
Provider-neutral core, Git reference provider, Draft operation log, finalization model + receipts, `services/draftd`, provider-neutral CLI/TUI, experimental providers (fs/jj/mercurial/pijul), v0.1.0 migration, public docs.

## Known limitations (v0.2.0)
- jj / Mercurial / Pijul providers are experimental (detection + capabilities).
- Filesystem provider is limited (status scan; no finalization).
- No cloud sync, hosted review, or real-time/CRDT collaboration.
- No Draft-native VCS; no cryptographic operation signing by default; no enterprise policy server.

## Future directions
Full jj/Mercurial/Pijul support; richer manual grouping & partial finalization; operation signing; optional sync; provider-identity mapping; deeper TUI.
