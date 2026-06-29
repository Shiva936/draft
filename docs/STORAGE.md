# Draft Storage Layout

Draft stores all its data inside `.draft/` at the root of your Git repository.

```
.draft/
├── config.toml
├── repo.toml
├── sessions/
├── objects/
│   └── blobs/
│       └── <sha256-hash>       # content-addressed file blobs
├── checkpoints/
│   └── <checkpoint-id>.json
├── verification/
│   └── <verification-id>.json
├── receipts/
│   └── <commit-hash>.json
└── logs/
    └── draft.log
```

---

## Files

### config.toml

Created by `draft start`. Stores per-repo Draft configuration.

```toml
version = 1
repo_id = "<uuid>"
default_verify_command = ""
created_at = "<rfc3339>"
```

### repo.toml

Created by `draft start`. Stores repo metadata at initialization time.

```toml
repo_root = "/path/to/repo"
git_dir = "/path/to/repo/.git"
initial_head = "<sha1>"
created_at = "<rfc3339>"
```

---

## Directories

### objects/blobs/

Content-addressed blob storage. Each file is stored by its SHA-256 hash. Blobs are shared across checkpoints — the same file content is stored once regardless of how many checkpoints reference it.

### checkpoints/

One JSON file per checkpoint. Created automatically before every `draft commit` and on demand. Used by `draft undo` to restore the working tree.

**Checkpoint schema:**
```json
{
  "checkpoint_id": "<uuid>",
  "session_id": "<uuid>",
  "repo_head": "<git-sha1>",
  "message": "Pre-commit: Fix login",
  "created_at": "<rfc3339>",
  "files": [
    {
      "path": "src/auth.rs",
      "content_hash": "<sha256>",
      "file_status": "Modified"
    }
  ]
}
```

### verification/

One JSON file per `draft verify` run.

**Evidence schema:**
```json
{
  "verification_id": "<uuid>",
  "command": "cargo test",
  "exit_code": 0,
  "status": "Passed",
  "started_at": "<rfc3339>",
  "finished_at": "<rfc3339>",
  "duration_ms": 4231,
  "stdout_summary": "...",
  "stderr_summary": ""
}
```

### receipts/

One JSON file per successful `draft commit`. Named by the Git commit hash.

**Receipt schema:**
```json
{
  "receipt_id": "<uuid>",
  "draft_version": "0.1.0",
  "repo_id": "<uuid>",
  "commit_hash": "<git-sha1>",
  "commit_message": "Fix login validation",
  "branch": "main",
  "head_before": "<git-sha1>",
  "head_after": "<git-sha1>",
  "included_files": ["src/auth.rs"],
  "excluded_files": [],
  "risk_summary": { "level": "Medium", "reasons": [] },
  "verification": { ... },
  "checkpoint_id": "<uuid>",
  "created_at": "<rfc3339>"
}
```

### logs/

Append-only log of Draft operations. Each line is timestamped.

```
[2024-01-01T00:00:00Z] Draft session initialized or resumed.
[2024-01-01T00:01:00Z] Checkpoint created: abc123 (Pre-commit: Fix login)
[2024-01-01T00:01:01Z] Verification run complete: cargo test -> Passed
```

---

## Notes

- `.draft/` is automatically excluded from Git tracking via `.git/info/exclude`.
- Do not manually edit files in `.draft/` — the format may change between versions.
- To reset Draft state entirely, delete `.draft/` and run `draft start` again.
