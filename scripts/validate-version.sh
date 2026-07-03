#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

failures=0

fail() {
  printf 'version check failed: %s\n' "$*" >&2
  failures=$((failures + 1))
}

normalize_version() {
  printf '%s' "${1#v}"
}

read_package_version() {
  local file="$1"
  awk -F '"' '/^version = / { print $2; exit }' "$file"
}

expected="${1:-}"
if [ -z "$expected" ]; then
  expected="$(read_package_version cli/Cargo.toml)"
fi
expected="$(normalize_version "$expected")"

if ! printf '%s' "$expected" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-.+][0-9A-Za-z.-]+)?$'; then
  fail "expected version '$expected' is not a semantic version"
fi

if command -v git >/dev/null 2>&1; then
  if tag="$(git describe --tags --exact-match 2>/dev/null)"; then
    tag_version="$(normalize_version "$tag")"
    if [ "$tag_version" != "$expected" ]; then
      fail "git tag '$tag' does not match expected version '$expected'"
    fi
  fi
fi

while IFS= read -r manifest; do
  package_name="$(awk -F '"' '/^name = / { print $2; exit }' "$manifest")"
  package_version="$(read_package_version "$manifest")"
  if [ -n "$package_name" ] && { [ "$package_name" = "draftd" ] || printf '%s' "$package_name" | grep -Eq '^draft-'; }; then
    if [ "$package_version" != "$expected" ]; then
      fail "$manifest package '$package_name' has version '$package_version', expected '$expected'"
    fi
  fi
done < <(git ls-files '*Cargo.toml')

if [ -f Cargo.lock ]; then
  while IFS='|' read -r package_name package_version; do
    if [ "$package_version" != "$expected" ]; then
      fail "Cargo.lock package '$package_name' has version '$package_version', expected '$expected'"
    fi
  done < <(
    awk -v expected="$expected" '
      /^name = / {
        name=$0
        sub(/^name = "/, "", name)
        sub(/"$/, "", name)
      }
      /^version = / {
        version=$0
        sub(/^version = "/, "", version)
        sub(/"$/, "", version)
        if (name == "draftd" || name ~ /^draft-/) {
          print name "|" version
        }
      }
    ' Cargo.lock
  )
fi

check_contains() {
  local file="$1"
  local pattern="$2"
  local description="$3"
  if [ -f "$file" ] && ! grep -Eq "$pattern" "$file"; then
    fail "$file does not contain expected $description for version '$expected'"
  fi
}

check_contains README.md "version-v${expected}" "README version badge"
check_contains README.md "Draft v${expected}|v${expected}" "README release version"
check_contains RELEASE_NOTES.md "v${expected}" "release notes version"
check_contains docs/installation.md "Draft v${expected}|v${expected}" "installation doc version"
check_contains docs/release-compliance.md "v${expected}" "release compliance version"

if [ "$failures" -ne 0 ]; then
  exit 1
fi

printf 'Draft version %s is consistent across release declarations.\n' "$expected"
