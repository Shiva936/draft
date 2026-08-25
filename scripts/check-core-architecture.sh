#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
core="$root_dir/core/src"
domains=(support contracts workspace task pack trust operation review read_model app)

allowed_for() {
  case "$1" in
    support) echo "support" ;;
    contracts) echo "contracts support" ;;
    workspace) echo "workspace contracts support" ;;
    task) echo "task contracts support workspace" ;;
    pack) echo "pack contracts support workspace" ;;
    trust) echo "trust contracts support workspace" ;;
    operation) echo "operation contracts support workspace" ;;
    review) echo "review contracts support workspace task pack trust" ;;
    read_model) echo "read_model contracts support workspace task pack trust operation review" ;;
    app) echo "app contracts support workspace task pack trust operation review read_model" ;;
  esac
}

for domain in "${domains[@]}"; do
  allowed=" $(allowed_for "$domain") "
  while IFS= read -r dependency; do
    [ -z "$dependency" ] && continue
    if [[ "$allowed" != *" $dependency "* ]]; then
      echo "Forbidden core dependency: $domain -> $dependency" >&2
      exit 1
    fi
  done < <(rg -o 'crate::(app|contracts|support|workspace|task|pack|trust|operation|review|read_model)' \
    "$core/$domain" --glob '*.rs' 2>/dev/null | sed 's/.*crate:://' | sort -u)
done

if [ -d "$core/presentation" ] || rg -q 'mod presentation|crate::presentation' "$core"; then
  echo "draft-core must not contain a presentation namespace." >&2
  exit 1
fi
if rg -n 'serde\((alias|untagged)|serde\([^)]*alias' "$core"; then
  echo "Compatibility serde readers are forbidden in canonical v1." >&2
  exit 1
fi
if rg -n '(^|/)(design|theme)\.rs$|pub (struct|enum) [A-Za-z]*(Theme|Color|Spacing|Typography)' "$core"; then
  echo "UI design tokens must not live in draft-core." >&2
  exit 1
fi
if rg -n 'use (draft_agui|draft_tui)|crate::(console|tui)' \
  "$core"/{support,contracts,workspace,task,pack,trust,operation,review,read_model}; then
  echo "Core domains must not import UI or service implementations." >&2
  exit 1
fi
if rg -n '^pub struct [A-Za-z0-9]*(Record|Manifest|Envelope|Store)\b' "$core/app"; then
  echo "Persisted domain models and stores must not be defined under app/." >&2
  exit 1
fi
if rg -n 'serde\(deny_unknown_fields\)|^(pub(\(crate\))? )?struct (ObjectStore|ObjectPack|WorkspaceEvents|WorkspaceEventLog|Scanner|Snapshotter|IgnoreMatcher|ReviewFile|ActionReceiptDraft|VerificationConfig|VerifyFile|RiskConfig)\b|^(pub(\(crate\))? )?enum RiskLevel\b' "$core/app" --glob '*.rs'; then
  echo "Authoritative contracts, storage implementations, and domain scanners must not be defined under app/." >&2
  exit 1
fi

echo "Core dependency allowlist and ownership checks passed."
