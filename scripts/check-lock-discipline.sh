#!/usr/bin/env bash
set -euo pipefail

# Enforce the locking rules that correctness depends on.
#
# Each rule here exists because breaking it produces a bug that testing does not
# reliably catch: a lost update under contention, a lock that outlives its
# owner, or a deadlock that only appears when two operations overlap. They are
# cheap to check statically and expensive to discover in production.

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
failed=0

report() {
  local message="$1"
  shift
  local hits
  if hits="$("$@" 2>/dev/null)"; then
    echo "$message" >&2
    echo "$hits" >&2
    failed=1
  fi
}

search_paths=("$root_dir/core/src" "$root_dir/cli/src" "$root_dir/console")
for service in "$root_dir"/services/*/src; do
  [ -d "$service" ] && search_paths+=("$service")
done

# ---------------------------------------------------------------------------
# 1. The stealable advisory lock is gone and stays gone.
#
#    It had a 30-second wall-clock stale takeover, so a live but slow holder
#    could have its lock stolen mid-compare-exchange and lose a committed
#    update. Reintroducing it anywhere would reintroduce that race.
# ---------------------------------------------------------------------------

if [ -f "$root_dir/core/src/support/lock.rs" ]; then
  echo "core/src/support/lock.rs is back; the stealable advisory lock was removed \
deliberately." >&2
  failed=1
fi

# Naming it in a comment to explain why it was removed is not a
# reintroduction, so only code lines count.
if hits="$(grep -rn --include='*.rs' -E '\bFileGuard\b' "${search_paths[@]}" \
  --exclude-dir=target 2>/dev/null | grep -vE ':[0-9]+:\s*(//|\*)')"; then
  echo "The stealable advisory lock has been reintroduced:" >&2
  echo "$hits" >&2
  failed=1
fi

# ---------------------------------------------------------------------------
# 2. Correctness locks target a stable sidecar, never the record.
#
#    Records are replaced by atomic rename, which creates a new inode. A lock
#    held on the record's own path stops protecting anything the moment a write
#    lands, so the critical section silently becomes no critical section.
# ---------------------------------------------------------------------------

report "A correctness lock targets a record rather than a stable .lock sidecar:" \
  grep -rn --include='*.rs' -E 'acquire_exclusive\([^)]*\.(json|toml|log|sqlite)' \
  "${search_paths[@]}" --exclude-dir=target

# ---------------------------------------------------------------------------
# 3. Nothing holding a Store's lock calls that Store's self-locking
#    compare_exchange.
#
#    ProcessFileLock is not reentrant, so this deadlocks against the caller's
#    own lock until the timeout. The guarded API exists precisely so the
#    in-section mutation is a different call.
# ---------------------------------------------------------------------------

if ! python3 - "$root_dir" <<'REENTRANCY_PY'
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])
roots = [root / "core" / "src", root / "cli" / "src"]
roots += [p for p in (root / "services").glob("*/src") if p.is_dir()]

# Track brace depth from the `with_locked_record` / `with_locked_attempt` call
# to its closing brace, and flag a self-locking compare_exchange inside it.
ENTER = re.compile(r"\.with_locked_(record|control|attempt)\s*\(")
FORBIDDEN = re.compile(r"\.compare_exchange\s*\(")

violations = []
for base in roots:
    for path in base.rglob("*.rs"):
        depth = None
        text = path.read_text(encoding="utf-8", errors="replace")
        for number, line in enumerate(text.splitlines(), start=1):
            if depth is None:
                if ENTER.search(line):
                    depth = line.count("{") - line.count("}")
                    if depth <= 0:
                        depth = None
                continue
            if FORBIDDEN.search(line):
                violations.append(
                    f"{path.relative_to(root)}:{number}: self-locking compare_exchange "
                    "inside a held critical section"
                )
            depth += line.count("{") - line.count("}")
            if depth <= 0:
                depth = None

if violations:
    print("A path holding a Store lock calls that Store's self-locking compare_exchange:")
    for violation in violations:
        print(f"  - {violation}")
    raise SystemExit(1)
REENTRANCY_PY
then
  failed=1
fi

# ---------------------------------------------------------------------------
# 4. Lock handles are non-inheritable.
#
#    A child that inherited a correctness-lock descriptor would keep the lock
#    held after its owner died, with nothing able to take it and nothing able
#    to release it. The implementation must keep saying so explicitly.
# ---------------------------------------------------------------------------

lock_module="$root_dir/core/src/support/process_lock.rs"
if [ ! -f "$lock_module" ]; then
  echo "core/src/support/process_lock.rs is missing." >&2
  failed=1
else
  if ! grep -q "O_CLOEXEC" "$lock_module"; then
    echo "The Unix lock no longer opens its descriptor close-on-exec." >&2
    failed=1
  fi
  if ! grep -q "HANDLE_FLAG_INHERIT" "$lock_module"; then
    echo "The Windows lock no longer marks its handle non-inheritable." >&2
    failed=1
  fi
  # A wall-clock takeover is the exact defect this type replaced. Taking a
  # lock over requires deleting the holder's lock file or judging it by its
  # mtime, so those are what is checked — not the word "elapsed", which the
  # perfectly legitimate acquisition timeout also uses.
  if grep -nE '\bremove_file\b|\bmodified\(\)|\bSTALE_AFTER\b' "$lock_module" \
    | grep -vE ':\s*(//|\*)'; then
    echo "The correctness lock has grown a staleness takeover; a live holder must \
never be displaced by elapsed time or have its lock file removed." >&2
    failed=1
  fi
fi

# ---------------------------------------------------------------------------
# 5. Journal records are written only through their owning Store.
#
#    A direct filesystem write to a journal bypasses the guarded state machine,
#    which is what makes two actors able to last-writer-wins a transition.
# ---------------------------------------------------------------------------

report "A journal record is written outside its owning Store:" \
  grep -rn --include='*.rs' -E 'write_(atomic|json)\([^)]*journal' \
  "${search_paths[@]}" --exclude-dir=target \
  --exclude=mutation_journal.rs

if [ "$failed" -ne 0 ]; then
  exit 1
fi
echo "Lock discipline checks passed."
