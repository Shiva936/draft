# PackWorkspace Protocol

Draft v0.3.4 pack workspaces are temporary, locally verifiable units of change. Canonical pack workspace identifiers use `pck_<id>`. A pack workspace is not permanent project state until `draft submit` finalizes it.

Required canonical fields:

- `schema_version`
- `pack_id`
- `name`
- `base_workspace_hash`
- `target_workspace_hash`
- `changes_hash`
- `risk_hash`
- `verify_hash`
- `receipt_hashes`
- `approval_state`
- `submit_state`

Path-bearing payloads must reject `.draft/`, absolute paths, traversal, symlink escapes, and paths outside the workspace root.
