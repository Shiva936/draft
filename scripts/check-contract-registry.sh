#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
registry="$root_dir/core/src/contracts/mod.rs"
production=(
  "$root_dir/core/src"
  "$root_dir/cli/src"
  "$root_dir/services"
  "$root_dir/console/src"
  "$root_dir/tui/src"
)

if rg -n '\bSCHEMA_VERSION\b' "${production[@]}" --glob '*.rs'; then
  echo "A global schema-version constant bypasses per-contract ownership." >&2
  exit 1
fi

if rg -n 'schema_version:\s*1([,}])|schema_version\s*(==|!=|<=|>=|<|>)\s*1' \
  "${production[@]}" --glob '*.rs'; then
  echo "Production code contains an ad-hoc schema-version literal." >&2
  exit 1
fi

if rg -n -U 'schema_version\s*\n?\s*(==|!=)\s*[^\n]*current_version' \
  "${production[@]}" --glob '*.rs'; then
  echo "Readers must use each contract's supported policy, not require its current write version." >&2
  exit 1
fi

entries="$(rg -c '^    [A-Za-z0-9_]+ => ' "$registry")"
policies="$(rg -c '^    [A-Za-z0-9_]+ => .*VersionPolicy::' "$registry")"
if [ "$entries" -eq 0 ] || [ "$entries" -ne "$policies" ]; then
  echo "Every closed ContractId entry must declare its own VersionPolicy." >&2
  exit 1
fi

if rg -n 'register_contract|ContractId[^\n]*(from_str|parse)|HashMap<[^>]*ContractId' \
  "${production[@]}" --glob '*.rs'; then
  echo "Runtime contract registration or string-selected production dispatch is forbidden." >&2
  exit 1
fi

cargo test --locked -p draft-core --lib contracts::tests::registry_is_closed_unique_and_v1_for_this_release
echo "Closed per-contract registry checks passed."
