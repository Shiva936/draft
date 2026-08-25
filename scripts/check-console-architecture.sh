#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
gateway="$root_dir/console/src/lib.rs"
daemon="$root_dir/services/draftd/src/lib.rs"

if rg -q 'draft-core|draft_core' "$root_dir/console/Cargo.toml" "$gateway"; then
  echo "Console gateway must not depend directly on draft-core." >&2
  exit 1
fi

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
rg -Fq 'valid_actions_for_label' "$root_dir/core/src/pack/lifecycle.rs"
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

echo "Console architecture and contract boundary checks passed."
