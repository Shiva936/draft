# Draft Checkpoint Model

A checkpoint is a snapshot of the working tree state captured before every `draft commit`. It is the basis for `draft undo`.

## Purpose

Checkpoints allow Draft to recover the exact file state before a commit was created. If a commit is wrong or something goes wrong, `draft undo` restores the working tree from the checkpoint.

## When checkpoints are created

- Automatically before every `draft commit`

## Schema

```json
{
  "checkpoint_id": "<uuid>",
  "session_id": "<uuid>",
  "repo_head": "<git-sha1>",
  "message": "Pre-commit: Fix login validation",
  "created_at": "<rfc3339>",
  "files": [
    {
      "path": "src/auth.rs",
      "content_hash": "<sha256>",
      "file_status": "Modified"
    },
    {
      "path": "src/new_feature.rs",
      "content_hash": "<sha256>",
      "file_status": "Added"
    },
    {
      "path": "src/old.rs",
      "content_hash": "<sha256>",
      "file_status": "Deleted"
    }
  ]
}
```

## Fields

| Field | Type | Description |
|---|---|---|
| `checkpoint_id` | UUID | Unique checkpoint identifier |
| `session_id` | UUID | Session that created this checkpoint |
| `repo_head` | String | Git HEAD at checkpoint creation time |
| `message` | String | Human-readable label |
| `created_at` | RFC3339 | When checkpoint was created |
| `files` | Array | All changed files and their stored content hashes |

## File status values

| Status | Meaning |
|---|---|
| `Modified` | File existed and was changed |
| `Added` | File is newly added |
| `Deleted` | File was deleted (content from HEAD is stored) |
| `Untracked` | File is untracked by Git |
| `Renamed` | File was renamed |
| `Copied` | File was copied |

## Blob storage

Each file's content is stored as a content-addressed blob in `.draft/objects/blobs/<sha256>`. Multiple checkpoints referencing the same file content share a single blob.

## Restore behavior

`draft undo` reads the latest checkpoint, shows what will be changed, and after confirmation:
- Writes stored blob content back to each file path for `Modified`, `Added`, `Untracked`
- Deletes files that had status `Added` (they did not exist before the checkpoint)
- Re-creates files for `Deleted` entries (they were deleted in the working tree)

## Limitations

- Only files tracked by `git status` at checkpoint time are captured.
- Untracked files that have never been `git add`-ed are not guaranteed to be captured.
- Checkpoints are not pruned automatically. Delete `.draft/checkpoints/` to clear them.
