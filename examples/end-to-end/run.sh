#!/usr/bin/env sh
# shellcheck source=../lib.sh
. "$(dirname "$0")/../lib.sh"
# The complete local lifecycle: a small project, a ChangePack, a sealed
# RevisionPack, governance, Promotion to a Baseline, Activity and the
# Promotion receipt — then the Console over it. Publication is a separate,
# provider-gated effect; see ../publication.
fresh_project
printf 'Draft end to end\n' > README.txt
printf 'fn main() {}\n' > main.rs
workspace_id="$(draft project init "$PWD" --json | top workspace_id)"
declare_passing_check

step "Milestone 1: open and seal"
change_pack="$(draft pack new "first feature" --scope main.rs README.txt --json | top id)"
printf 'fn main() { println!("hi"); }\n' > main.rs
revision_pack="$(draft pack revision seal "$change_pack" --json | top id)"

step "Milestone 2: govern the exact RevisionPack"
set -- $(govern "$revision_pack")
decision="$1"
gate="$2"
draft pack gates list "$change_pack" "$revision_pack"

step "Milestone 3: Promotion → Baseline"
draft promote "$change_pack" "$revision_pack" --gate "$gate" --decision "$decision"
draft baseline show

step "Milestone 4: what happened, and what was attested"
draft activity list
draft baseline receipts
draft pack receipts "$change_pack"

step "Milestone 5: the Console, bounded by its readiness line"
log="$EXAMPLE_ROOT/console.log"
draft console web --no-open --port 0 --project "$workspace_id" > "$log" 2>&1 &
console_pid=$!
# `draft` is a shell function, so $! is the subshell running it; the Console
# is that subshell's child, and it is the one that must see Ctrl-C.
cleanup() {
  pkill -INT -P "$console_pid" 2>/dev/null || true
  wait "$console_pid" 2>/dev/null || true
  draft daemon stop > /dev/null 2>&1 || true
}
trap cleanup EXIT
url=""
waited=0
while [ -z "$url" ]; do
  url="$(sed -n 's/.*Open this URL in your browser: \(http:\/\/127\.0\.0\.1:[0-9]*\/#bootstrap=[0-9a-f]*\).*/\1/p' "$log" | head -n 1)"
  [ -n "$url" ] && break
  kill -0 "$console_pid" 2>/dev/null || { cat "$log" >&2; echo "the Console exited early" >&2; exit 1; }
  waited=$((waited + 1))
  [ "$waited" -le 900 ] || { cat "$log" >&2; echo "the Console never became ready" >&2; exit 1; }
  sleep 0.2
done
origin="${url%%/#*}"
echo "Console ready at $origin"
if command -v curl > /dev/null 2>&1; then
  curl -fsS -o /dev/null "$origin/" && echo "The Console answered on $origin"
fi
