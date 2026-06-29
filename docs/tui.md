# TUI

`draft review` launches an interactive terminal UI when attached to a terminal (use `--no-ui` to force text output, or `--yes` to approve non-interactively).

The TUI consumes the same provider-neutral APIs as the CLI and never calls a provider directly. Status refreshes prefer `draftd` when it is running and fall back to embedded core mode; v0.2.0 review/finalize actions use embedded core because the daemon exposes only safe read endpoints. It shows workspace/provider status, change groups, risk, verification, and review state, and lets you:

- `a` — approve all change groups
- `f` — finalize (enter a message, then Enter)
- `r` — refresh · `q` — quit

After finalizing it shows the resulting receipt id.
