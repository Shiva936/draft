#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
typescript="$root_dir/console/web/src/generated-contracts.ts"
schema="$root_dir/proto/schemas/console-http.schema.json"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/draft-console-contracts.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

cargo run --locked -p draft-ipc --bin generate-console-contracts --manifest-path "$root_dir/Cargo.toml" -- \
  "$scratch/generated-contracts.ts" "$scratch/console-http.schema.json"

if ! cmp -s "$scratch/generated-contracts.ts" "$typescript" \
  || ! cmp -s "$scratch/console-http.schema.json" "$schema"; then
  echo "Generated Console DTOs drifted from Rust types; run cargo run -p draft-ipc --bin generate-console-contracts." >&2
  exit 1
fi

echo "Generated Console DTOs match Rust types."
