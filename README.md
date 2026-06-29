# Draft

> Trust your code before you commit.

Draft is a CLI-first, TUI-assisted pre-commit trust layer for Git repositories. It helps developers review, verify, compose, and safely commit human + AI-assisted code changes before they enter Git history.

---

## The problem

You used an AI coding tool for an hour. Many files changed. Some changes are good. Some are risky. Some are unrelated. You don't fully know what should be committed.

Git shows you the final diff. It doesn't explain intent, risk, grouping, or verification. Draft fills that gap.

---

## Quickstart

```bash
# In any Git repository
draft start

# Code normally with your tools (Cursor, Claude Code, Vim, etc.)

draft review          # Open interactive review TUI
draft verify          # Run tests and record evidence
draft commit -m "Fix login validation"

# If anything goes wrong
draft undo
```

---

## Commands

| Command | Description |
|---|---|
| `draft start` | Initialize Draft in the current repo |
| `draft status` | Show repo and risk summary |
| `draft review` | Review changes and open TUI |
| `draft verify` | Run and record verification |
| `draft commit -m "..."` | Create a safe Git commit |
| `draft undo` | Restore last checkpoint |

See [docs/COMMANDS.md](docs/COMMANDS.md) for full reference.

---

## Install

### From source (requires Rust)

```bash
git clone https://github.com/your-org/draft
cd draft
cargo install --path cli
```

### Verify

```bash
draft --version
```

---

## Storage

Draft stores all data locally in `.draft/` inside your repo. Nothing is sent to any server. See [docs/STORAGE.md](docs/STORAGE.md) for the full layout.

## Safety

Draft creates a checkpoint before every commit. `draft undo` restores it. See [docs/SAFETY.md](docs/SAFETY.md) for guarantees and limitations.

---

## License

MIT
