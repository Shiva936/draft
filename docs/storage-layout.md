# Storage layout

## Workspace-local (portable truth) — `.draft/`
```
.draft/
  config.toml        # policy (provider, verification, risk, finalization)
  workspace.json     # identity (workspace id, provider binding)
  identity.json      # optional local actor identity
  operations/        # append-only log + index.json
  changes/           # change_<id>.json + groups.json
  reviews/           # review_<id>.json
  checkpoints/       # checkpoint_<id>.json
  verification/      # result_<id>.json + logs/
  receipts/          # receipt_<id>.json + index.json
  locks/             # advisory lock files
  objects/blobs/     # content-addressed blobs (when used)
  backup/            # pre-migration backups
```
All structured writes are atomic (temp file + fsync + rename). Indexes are rebuildable from their source records.

## User-local (machine state) — not portable
```
~/.config/draft/      identity.toml, config.toml
~/.local/state/draft/ workspace-index.json, provider cache, logs/, draftd.pid,
                      draftd.sock
```
`.draft/` is excluded from provider history by default (for Git, via
`.git/info/exclude`).
