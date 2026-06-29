# Draft

> Review, verify, and finalize your work — before it becomes a commit.

Draft is a **local-first, provider-neutral collaboration workspace**. It helps developers and AI agents group, review, risk-score, verify, checkpoint, and **finalize** changes before they become native version-control objects.

Draft is not a Git replacement and not only a Git client: it owns pre-finalization state, while a pluggable **provider** owns native VCS state. Git is the complete reference provider; jj, Mercurial, Pijul, and a filesystem provider ship as experimental scaffolds.

---

## Quickstart

```bash
cd my-git-repo
draft workspace detect      # confirm the provider (Git)
draft workspace init        # create .draft/ (excluded from history)

# ... edit code with your tools ...

draft status                # provider-neutral change/risk/verification summary
draft review --yes          # approve change groups (TUI launches when interactive)
draft verify "cargo test"   # run verification (shown before it runs)
draft commit -m "Fix login" # finalize into a Git commit + write a receipt
draft receipt list          # receipt maps Draft change → commit SHA
draft undo                  # safely reverse the last finalization
```

`draft commit` is a compatibility command that routes to the **finalization**
engine.

---

## Commands

| Command | Description |
|---|---|
| `draft start` / `draft workspace init` | Initialize a Draft workspace |
| `draft workspace detect` | Detect the provider for this path |
| `draft status` | Provider-neutral status summary |
| `draft review [--yes]` | Review / approve change groups |
| `draft verify [CMD]` | Run and record verification |
| `draft commit -m "..."` | Finalize reviewed changes |
| `draft undo` | Reverse the last finalization |
| `draft provider list` | List providers and capabilities |
| `draft receipt list/show <id>` | Inspect durable receipts |
| `draft service start/stop/status` | Manage the optional `draftd` daemon |

Full documentation is in [docs/](docs/README.md).

---

## Architecture

```
cli / tui → (draftd over IPC, or embedded) → core::App → core engines → core::vcs registry → providers/{git,fs,jj,mercurial,pijul}
```

Core is provider-neutral; Git lives only in `providers/git`. See
[docs/architecture.md](docs/architecture.md).

---

## Install (from source, Rust 1.75+)

```bash
cargo build --release
# binaries: target/release/draft and target/release/draftd
draft --version    # 0.2.0
```

---

## Storage & safety

Draft stores everything locally in `.draft/` (atomic writes; rebuildable indexes). Nothing is uploaded by default. `.draft/` is excluded from provider history by default. A pre-finalization checkpoint enables `draft undo`. See [docs/storage-layout.md](docs/storage-layout.md) and [docs/security.md](docs/security.md).

## License

MIT or Apache 2.0
