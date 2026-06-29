# Draft CLI Command Reference

## draft start

Initialize or resume Draft for the current Git repository.

```
draft start
```

Run this once per repo before using any other Draft commands. Creates `.draft/` in the repo root and excludes it from Git tracking.

**Output:** Confirms repo name, branch, HEAD, and Git identity.

---

## draft status

Show a summary of current working tree state.

```
draft status
draft status --json
```

**Flags:**
- `--json` — emit structured JSON output

**Output:** Branch, HEAD, changed file count, risk level, last checkpoint, verification status.

---

## draft review

Analyze uncommitted changes and open the review interface.

```
draft review
draft review --no-ui
draft review --json
```

**Flags:**
- `--no-ui` — print text summary instead of launching TUI
- `--json` — emit structured JSON output

**TUI controls:**
- `j/k` or `↑/↓` — move between change groups
- `Space` — toggle include/exclude a group
- `d` or `Enter` — view diff for selected group
- `t` — run verification command
- `c` — open commit dialog
- `q` or `Esc` — quit

---

## draft verify

Run and record a verification command (tests, linters, etc.).

```
draft verify
draft verify "cargo test"
draft verify "go test ./..."
draft verify --json
```

If no command is given, Draft infers one based on project type:

| File present | Inferred command |
|---|---|
| `Cargo.toml` | `cargo test` |
| `go.mod` | `go test ./...` |
| `package.json` | `npm test` |
| `pyproject.toml` | `pytest` |
| `Makefile` | `make test` |

Evidence is stored in `.draft/verification/`.

---

## draft commit

Create a Git commit from reviewed changes.

```
draft commit -m "Fix login validation"
draft commit -m "message" --yes
draft commit -m "message" --no-verify
draft commit -m "message" --json
```

**Flags:**
- `-m, --message` — commit message (required)
- `--yes` — skip confirmation prompt
- `--no-verify` — skip verification evidence check
- `--json` — emit structured JSON output

**Flow:**
1. Detects repo and opens storage
2. Analyzes and groups changes
3. Checks for conflicts (blocks if found)
4. Creates checkpoint
5. Stages included files
6. Creates Git commit
7. Saves receipt to `.draft/receipts/`

---

## draft undo

Restore the working tree to the last checkpoint.

```
draft undo
draft undo <checkpoint-id>
```

Works from any subdirectory within the Git repository. Shows what will be restored and asks for confirmation before applying.

---

## Global flags

These flags are not yet supported in v0.1.0:

```
--config    custom config path
--repo      explicit repo path
--verbose   debug output
```

---

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Error (printed to stderr) |
