# Path Safety Protocol

All content paths must be workspace-relative and pass the central path guard.
Reject `.draft/`, `..`, absolute paths, Windows drive/UNC prefixes, symlink
escapes into `.draft/`, and paths outside the workspace root.
