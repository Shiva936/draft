# Workspace model

A workspace is a directory containing `.draft/`, bound to exactly one provider.

- `.draft/workspace.json` — portable identity: workspace id, provider id, provider root (relative), Draft version, optional `migrated_from`.
- `.draft/config.toml` — policy: provider binding, `[verification]`, `[risk]`, `[finalization]`.

`draft workspace detect` reports the detected provider and capabilities. `draft workspace init` creates the layout, binds the provider, excludes `.draft/` from provider history, and appends `WorkspaceInitialized`. Opening a v0.1.0 workspace migrates it automatically (see migration notes).
