# Shared helpers for the examples. Sourced, never run.
#
# Every example works in a fresh temporary project with its own global store,
# so running one never touches your projects or your Draft settings.
set -eu

DRAFT_BIN="${DRAFT_BIN:-draft}"

# Call the Draft binary under test by the name a reader types.
draft() {
  "$DRAFT_BIN" "$@"
}

# A top-level string field of `--json` output.
top() {
  sed -n "s/^  \"$1\": \"\([^\"]*\)\".*/\1/p" | head -n 1
}

step() {
  printf '\n==> %s\n' "$*"
}

# A fresh project directory, with an isolated global store unless the caller
# already chose one.
fresh_project() {
  EXAMPLE_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/draft-example.XXXXXX")"
  export DRAFT_GLOBAL_HOME="${DRAFT_GLOBAL_HOME:-$EXAMPLE_ROOT/global}"
  mkdir -p "$EXAMPLE_ROOT/project"
  cd "$EXAMPLE_ROOT/project"
}

# A check the project declares itself, so evidence has something to verify.
declare_passing_check() {
  cat > .draft/verify.toml <<'TOML'
schema_version = 1

[[checks]]
name = "always"
enabled = true

[checks.command]
program = "true"
args = []
TOML
}

# Evidence → assessment → gate → decision for one RevisionPack. Prints the
# approving decision id and the satisfied gate id on stdout.
govern() {
  revision_pack="$1"
  draft pack evidence run "$revision_pack" > /dev/null
  draft pack assess "$revision_pack" --risk low --rationale "reviewed in the example" > /dev/null
  gate="$(draft pack gates evaluate "$revision_pack" --json | top id)"
  decision="$(draft pack decide "$revision_pack" --approve --gate "$gate" --json | top id)"
  printf '%s %s\n' "$decision" "$gate"
}
