# Draft Receipt Model

Every successful `draft commit` produces a receipt stored in `.draft/receipts/<commit-hash>.json`.

## Purpose

The receipt is a local, append-only record of what was committed, when, by whom, with what risk assessment, and what verification evidence was present. It is not sent anywhere. It is not required for Git to function.

## Schema

```json
{
  "receipt_id": "<uuid>",
  "draft_version": "0.1.0",
  "repo_id": "<uuid>",
  "session_id": "<uuid>",
  "commit_hash": "<git-sha1>",
  "commit_message": "Fix login validation",
  "branch": "main",
  "head_before": "<git-sha1>",
  "head_after": "<git-sha1>",
  "included_files": ["src/auth.rs", "src/login.rs"],
  "excluded_files": ["debug.rs"],
  "risk_summary": {
    "level": "Medium",
    "reasons": [
      { "code": "SECURITY_CODE", "message": "Touched auth.rs", "path": "src/auth.rs" }
    ]
  },
  "verification": {
    "verification_id": "<uuid>",
    "command": "cargo test",
    "exit_code": 0,
    "status": "Passed",
    "started_at": "<rfc3339>",
    "finished_at": "<rfc3339>",
    "duration_ms": 4231,
    "stdout_summary": "test result: ok. 12 passed",
    "stderr_summary": ""
  },
  "checkpoint_id": "<uuid>",
  "identity": {
    "name": "Alice",
    "email": "alice@example.com",
    "source": "GitConfig"
  },
  "coauthors": [],
  "created_at": "<rfc3339>"
}
```

## Fields

| Field | Type | Description |
|---|---|---|
| `receipt_id` | UUID | Unique receipt identifier |
| `draft_version` | String | Draft version that created this receipt |
| `repo_id` | UUID | Repo identifier from `.draft/config.toml` |
| `commit_hash` | String | Git commit SHA1 |
| `commit_message` | String | The commit message |
| `branch` | String? | Branch name at commit time |
| `head_before` | String | Git HEAD before commit |
| `head_after` | String | Git HEAD after commit |
| `included_files` | Array | Paths staged and committed |
| `excluded_files` | Array | Paths excluded from this commit |
| `risk_summary` | Object | Risk level and reasons at commit time |
| `verification` | Object? | Verification evidence if present |
| `checkpoint_id` | UUID | Checkpoint created before this commit |
| `identity` | Object? | Git identity of committer |
| `created_at` | RFC3339 | Timestamp of receipt creation |

## Notes

- Receipts are never deleted by Draft automatically.
- If receipt writing fails after a successful commit, Draft warns but does not roll back.
- Receipts are human-readable JSON and can be inspected with any JSON viewer.
