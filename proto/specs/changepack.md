# Changepack Protocol

Draft v0.3.3 changepacks are temporary, locally verifiable units of change.
Canonical changepack identifiers use `pck_<id>`. A changepack is not permanent
project state until `draft save` finalizes it.

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
- `save_state`

Path-bearing payloads must reject `.draft/`, absolute paths, traversal, symlink
escapes, and paths outside the workspace root.
