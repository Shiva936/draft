#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scan=(
  "$root_dir/core/src"
  "$root_dir/cli/src"
  "$root_dir/services"
  "$root_dir/console/application/src"
  "$root_dir/console/tui/src"
  "$root_dir/console/web/src"
  "$root_dir/proto"
  "$root_dir/docs"
)

if rg -n --pcre2 '\b[A-Za-z][A-Za-z0-9]*(?:V[2-9][0-9]*|_v[2-9][0-9]*)\b|[A-Za-z0-9_-]+-v[0-9]+\.schema\.json|draft-[a-z0-9-]+/[0-9]+' \
  "${scan[@]}" --glob '!**/console/dist/**' --glob '!**/node_modules/**' --glob '!**/package-lock.json' \
  | rg -v 'Uuid::new_v4|UUID `new_v4`'; then
  echo "Versioned Draft-owned identifier or protocol name found." >&2
  exit 1
fi

if find "$root_dir/proto/schemas" -maxdepth 1 -type f \
  \( -name '*-v[0-9]*' -o -name '*_v[0-9]*' -o -name '*v[0-9]*.schema.json' \) \
  | rg -q .; then
  echo "Versioned Draft schema filename found." >&2
  exit 1
fi

echo "Draft-owned contract names are stable and unversioned."
