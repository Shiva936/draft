# CLI

`draft` is provider-neutral and service-aware (prefers `draftd` when running, falls back to embedded mode for safe commands).

## Commands
| Command | Purpose |
| --- | --- |
| `draft start` | initialize a workspace (alias for `workspace init`) |
| `draft status [--json]` | provider-neutral change/risk/verification summary |
| `draft review [--yes] [--no-ui] [--json]` | open a review session / approve |
| `draft verify [CMD] [--json]` | run verification commands (shown before running) |
| `draft commit -m MSG [--no-verify] [--allow-high-risk] [--json]` | finalize |
| `draft undo [--json]` | reverse the last finalization (safe where supported) |
| `draft checkpoint [-m MSG]` | create a provider checkpoint |
| `draft service start\|stop\|status` | manage the optional daemon |
| `draft provider list\|status [--json]` | inspect providers/capabilities |
| `draft workspace init\|detect [--provider P] [--experimental]` | lifecycle |
| `draft receipt list\|show <id> [--json]` | inspect receipts |

`draft commit` is a **compatibility command** that routes to the finalization engine. Output reads "Finalized N Draft change(s) into <kind> <id>".

Exit code `2` indicates a policy gate blocked the action (review/verification/risk/conflict).

`draft service start` registers the current workspace when it is run inside an initialized Draft workspace. Outside a workspace, service startup still succeeds and commands continue to use embedded fallback where safe.
