#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
gateway="$root_dir/console/src/lib.rs"
daemon="$root_dir/services/draftd/src/lib.rs"
application="$root_dir/console/application"
tui="$root_dir/console/tui"

if rg -q 'draft-core|draft_core' "$root_dir/console/Cargo.toml" "$gateway"; then
  echo "Console gateway must not depend directly on draft-core." >&2
  exit 1
fi

if rg -q 'draft-core|draft_core' "$application/Cargo.toml" "$application/src" "$tui/Cargo.toml" "$tui/src"; then
  echo "Console application client and TUI must not depend directly on draft-core." >&2
  exit 1
fi
if rg -n '\.draft/|\.draft\\|std::fs|walkdir|Command::new|std::process::Command' "$tui/src"; then
  echo "Console TUI must not inspect project state or spawn command-line tools." >&2
  exit 1
fi
if rg -n 'PackLifecycle|valid_actions_for_label|submit_readiness|fn .*next_safe' "$tui/src"; then
  echo "Console TUI must not infer lifecycle, readiness, or next-safe-actions." >&2
  exit 1
fi
rg -Fq '"console.handshake"' "$daemon"
rg -Fq '"console.snapshot"' "$daemon"
rg -Fq '"console.action.invoke"' "$daemon"
rg -Fq 'pub const CONSOLE_PROTOCOL_MAJOR' "$root_dir/services/ipc/src/console_application.rs"

methods="$(sed -n 's/^[[:space:]]*"[^"]*" => "\([^"]*\)",$/\1/p' "$gateway" | sort -u)"
while IFS= read -r method; do
  [ -z "$method" ] && continue
  if ! rg -Fq "\"$method\"" "$daemon"; then
    echo "Console mutation has no typed daemon operation: $method" >&2
    exit 1
  fi
done <<< "$methods"

rg -Fq 'pub const IPC_PROTOCOL: &str = "draft-ipc"' "$root_dir/services/ipc/src/protocol.rs"
rg -Fq '.route("/api/v1/' "$gateway"
rg -Fq 'valid_actions_for_label' "$root_dir/core/src/dcg/revision.rs"
if rg -Fq '/api/console/v1' "$root_dir/console/src" "$root_dir/console/web/src"; then
  echo "Retired Console routes remain in active source." >&2
  exit 1
fi
if rg -Fq 'schema_version: number' "$root_dir/console/web/src"; then
  echo "Generated TypeScript must use the literal schema_version type 1." >&2
  exit 1
fi
for retired in "services/a""gui" "draft-a""gui" "draft_a""gui"; do
  if rg -n -F "$retired" "$root_dir" \
    --hidden \
    --glob '!.git/**' \
    --glob '!target/**' \
    --glob '!console/web/node_modules/**' \
    --glob '!scripts/check-console-architecture.sh' \
    --glob '!scripts/check-docs-consistency.sh' \
    --glob '!scripts/check-core-architecture.sh'; then
    echo "Retired Console package/path reference remains: $retired" >&2
    exit 1
  fi
done

# --- WP7: the Console presents the model, and never re-decides it ---

console_sources=(
  "$root_dir/console/src"
  "$root_dir/console/web/src"
  "$root_dir/console/application/src"
  "$tui/src"
)

# A domain vocabulary belongs to whoever contributed it. The Console renders the
# ids it is handed; naming one in its own source would bake a domain into the
# platform and break the promise that an unknown domain still displays.
if rg -n --glob '!*.test.*' -e 'draft\.(text|software|language|agent|filesystem)[a-z.-]*/' "${console_sources[@]}"; then
  echo "Console source names a contributed identifier; render what the model supplies instead." >&2
  exit 1
fi

# An observation token is transient adapter fencing. It identifies nothing a
# person can act on, and displaying one invites treating it as a handle.
if rg -n 'observation_token|observationToken' "${console_sources[@]}"; then
  echo "Console must never render an observation token." >&2
  exit 1
fi

# Coverage domains are opaque and scoped to their adapter binding. Describing
# one as a folder or subdirectory asserts a hierarchy no scheme has to have.
if rg -in 'subdirector|sub-director|folder path|directory tree' "${console_sources[@]}"; then
  echo "Coverage wording implies a path hierarchy that opaque domains do not have." >&2
  exit 1
fi

# A rollback that is not `Complete` did not restore the target, and saying so
# is the single most consequential thing this UI can get wrong.
if rg -in 'fully restored|successfully restored|restore complete' "${console_sources[@]}"; then
  echo "Console claims a restoration outcome the model may not support." >&2
  exit 1
fi

# Classification is set-valued. A single exclusive kind on a resource would
# discard a correct assignment from a second, equally correct classifier.
if rg -n 'artifact_kind|artifactKind|resource_kind' "${console_sources[@]}"; then
  echo "Console renders a single exclusive classification; classes are a list." >&2
  exit 1
fi

echo "Console architecture and contract boundary checks passed."
