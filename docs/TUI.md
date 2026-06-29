# Draft TUI Guide

`draft review` launches an interactive terminal UI when run in an interactive terminal. Use `draft review --no-ui` to get text output instead.

## Layout

```
┌─────────────────────────────────────────────────────────────┐
│  Draft v0.1.0  │  branch: main  │  HEAD: 9af31c2  │  Alice  │
├────────────────────────┬────────────────────────────────────┤
│  Change Groups         │  Group Detail / Diff / Verify       │
│                        │                                      │
│  > [x] Source changes  │  Description: ...                   │
│    [ ] Test updates    │  Changed Files:                      │
│    [x] Config changes  │    • src/auth.rs                     │
│                        │  Risk: HIGH                          │
│                        │    ⚠ Touched auth.rs                │
│                        │  Verification: PASSED (cargo test)  │
├────────────────────────┴────────────────────────────────────┤
│  Space: Toggle  D: Diff  T: Verify  C: Commit  Q: Quit      │
└─────────────────────────────────────────────────────────────┘
```

## Keybindings

| Key | Action |
|---|---|
| `j` / `↓` | Move selection down |
| `k` / `↑` | Move selection up |
| `Space` | Toggle include/exclude selected group |
| `d` / `Enter` | View diff for selected group |
| `t` | Run verification command |
| `c` | Open commit dialog |
| `q` / `Esc` | Quit without committing |

### In diff view

| Key | Action |
|---|---|
| `j` / `↓` | Scroll diff down |
| `k` / `↑` | Scroll diff up |
| `Enter` / `Esc` / `d` | Return to review |

### In commit dialog

| Key | Action |
|---|---|
| Type | Enter commit message |
| `Backspace` | Delete character |
| `Tab` / `Enter` | Confirm message, move to commit action |
| `y` | Execute commit (after message confirmed) |
| `Esc` | Cancel and return to review |

## Change group panel

Each group shows:
- `[x]` included / `[ ]` excluded toggle
- Group title
- Risk badge: `LOW` (green) / `MED` (yellow) / `HIGH` (red) / `BLOCKED` (magenta)

Groups default to included. Toggle with `Space` to exclude from the commit.

## Detail panel

Shows for the selected group:
- Description
- List of changed files
- Risk reasons
- Verification status

Press `d` to switch to diff view for the selected group.

## Verification from TUI

Press `t` to run the inferred or configured verification command. The result is shown in the detail panel and recorded in `.draft/verification/`. It is attached to the commit receipt when you commit.

## Commit from TUI

Press `c` to open the commit dialog:
1. Type a commit message
2. Press `Tab` or `Enter` to confirm the message
3. Review the included/excluded file summary
4. Press `y` to execute the commit

On success, a popup shows the commit hash. Press `Enter` to exit Draft.

## Fallback

If stdout is not a terminal (e.g. piping, CI), `draft review` automatically falls back to text mode. Use `--no-ui` to force text mode explicitly.
