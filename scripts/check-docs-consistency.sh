#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
documents=(
  "$root_dir/README.md"
  "$root_dir/CHANGELOG.md"
  "$root_dir/CONTRIBUTING.md"
  "$root_dir/RELEASE_NOTES.md"
  "$root_dir/ROADMAP.md"
  "$root_dir/docs"
  "$root_dir/proto/specs"
)

if rg -n 'services/agui|draft config (get|set|unset) identity\.|draft identity (status|set)|draft hook get|settings/identity' \
  "${documents[@]}" --glob '*.md'; then
  echo "Documentation contains an obsolete path, command, or configuration example." >&2
  exit 1
fi

while IFS=: read -r source line match; do
  link="${match#*](}"
  link="${link%)}"
  link="${link#<}"
  link="${link%>}"
  case "$link" in
    http://*|https://*|mailto:*|'') continue ;;
  esac
  link="${link%%#*}"
  link="${link%%\?*}"
  [ -z "$link" ] && continue
  if [[ "$link" = /* ]]; then
    target="$root_dir$link"
  else
    target="$(dirname "$source")/$link"
  fi
  if [ ! -e "$target" ]; then
    echo "$source:$line links to missing local target: $link" >&2
    exit 1
  fi
done < <(rg --no-heading -n -o '\]\(([^)#]+)' "${documents[@]}" --glob '*.md')

echo "Documentation commands, paths, and local links are consistent."
