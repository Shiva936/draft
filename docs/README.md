# Draft documentation

Draft is a **local-first, provider-neutral collaboration workspace** that helps developers and AI agents review, verify, group, checkpoint, and finalize work *before* it becomes a native version-control object.

Draft is not a Git replacement and not only a Git client. It owns pre-finalization collaboration state; the selected **provider** owns native version-control state. Git is the first complete provider; jj, Mercurial, Pijul, and a filesystem provider ship as experimental scaffolds.

## Start here
- [overview](overview.md) — what Draft is and is not
- [getting-started](getting-started.md) — install and first workflow
- [concepts](concepts.md) — workspace, provider, change, finalization, receipt
- [architecture](architecture.md) — how the pieces fit together
- [cli](cli.md) / [tui](tui.md) — interfaces
- [services](services.md) — the optional `draftd` daemon

## Models
- [workspace-model](workspace-model.md) · [change-model](change-model.md) · [review-model](review-model.md) · [risk-engine](risk-engine.md) · [verification](verification.md) · [checkpointing](checkpointing.md) · [finalization](finalization.md) · [receipts](receipts.md) · [operation-log](operation-log.md) · [identity](identity.md) · [conflict-handling](conflict-handling.md)

## Providers & storage
- [vcs-providers](vcs-providers.md) · [git-provider](git-provider.md) · [storage-layout](storage-layout.md) · [security](security.md)

## Project
- [collaboration](collaboration.md) · [roadmap](roadmap.md) · [migration/v0.1-to-v0.2](migration/v0.1-to-v0.2.md) · [adr/](adr/) · specs under prd/ tdd/ nfrd/ blueprint/ srs/
