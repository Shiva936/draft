#!/usr/bin/env bash
set -euo pipefail

# Boundary checks for the Draft platform / extension split.
#
# Three properties are enforced here:
#
#   1. Draft Core contains no production decision logic keyed on a named
#      programming language, file extension, software ecosystem, toolchain, or
#      named domain-specific verifier. Domain knowledge is contributed data.
#   2. Dependencies point one way: extension-contract <- core <- extension-service
#      <- draftd/CLI/Console, and `/extensions/` depends on the portable format
#      crate alone.
#   3. `/extensions/` could be lifted into a standalone `draft-extensions`
#      repository without touching an implementation.
#
# The language check is deliberately targeted rather than a blanket string ban:
# it looks for the specific shapes this refactor removed, and defers the
# behavioural half to tests (core/tests/no_extension_capabilities.rs and the
# contribution-driven tests in core/src/evidence/).

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
allowlist="$root_dir/scripts/core-language-allowlist.txt"
failed=0

# Report every hit that is not covered by a reviewed allowlist entry.
report() {
  local description="$1"
  shift
  local hits
  hits="$("$@" || true)"
  [ -z "$hits" ] && return 0

  local surviving=""
  while IFS= read -r hit; do
    [ -z "$hit" ] && continue
    local allowed=0
    while IFS= read -r entry; do
      case "$entry" in ''|'#'*) continue ;; esac
      local path="${entry%%:*}"
      local pattern="${entry#*:}"
      if [[ "$hit" == *"$path"* && "$hit" == *"$pattern"* ]]; then
        allowed=1
        break
      fi
    done < "$allowlist"
    [ "$allowed" -eq 0 ] && surviving+="$hit"$'\n'
  done <<< "$hits"

  if [ -n "${surviving//[$'\n' ]/}" ]; then
    echo "$description" >&2
    echo "$surviving" >&2
    failed=1
  fi
}

core_src="$root_dir/core/src"

# Every Rust tree that ships as part of the Draft platform. Domain knowledge is
# forbidden in all of them, not only in Core: the browser's language table
# survived an earlier gate precisely because the scan stopped at `core/src`.
platform_rust=(
  "$root_dir/core/src"
  "$root_dir/sdk/extension-contract/src"
  "$root_dir/services/extension-service/src"
  "$root_dir/services/ipc/src"
  "$root_dir/services/draftd/src"
  "$root_dir/console/src"
  "$root_dir/console/application/src"
  "$root_dir/console/tui/src"
  "$root_dir/cli/src"
)

# The Console frontend. Generated contracts and test/e2e files are excluded:
# naming a language in a fixture is expected test knowledge, not production
# semantics.
web_src="$root_dir/console/web/src"
web_globs=(
  --glob '!*.test.ts'
  --glob '!*.test.tsx'
  --glob '!generated-contracts.ts'
  --glob '!**/e2e/**'
)

# Rust production source only: everything from the first `#[cfg(test)]` to the
# end of a file is a test module, and a fixture that names a language is
# expected rather than forbidden.
production_rust() {
  local directory
  for directory in "$@"; do
    [ -d "$directory" ] || continue
    while IFS= read -r file; do
      awk -v f="${file#"$root_dir/"}" \
        '/^#\[cfg\(test\)\]/ { stop = 1 } !stop { print f ":" FNR ":" $0 }' "$file"
    done < <(find "$directory" -name '*.rs' | sort)
  done
}

# `report` runs a command; these two feed it a pre-filtered corpus instead of a
# directory, so the test-module and generated-file exclusions apply uniformly.
scan_rust() {
  production_rust "${platform_rust[@]}" | rg -n --pcre2 "$1" | sed 's/^[0-9]*://'
}
scan_web() {
  rg -n --pcre2 "$1" "$web_src" "${web_globs[@]}"
}

# --- 1. The Draft platform owns no software-domain knowledge ----------------

# Toolchain and analyzer names. `clippy::` is a lint attribute and
# `eslint-disable` a lint pragma; neither is a feature Draft implements.
toolchains='\b(cargo|rustfmt|pytest|eslint|npm|yarn|pnpm|gradle|maven|rust-analyzer)\b|\bclippy\b(?!::)'
report "Draft must not name a toolchain or analyzer; contribute it instead:" \
  scan_rust "$toolchains"
report "The Console must not name a toolchain or analyzer; contribute it instead:" \
  scan_web "(?!.*(eslint-disable|eslint-enable))(?:$toolchains)"

# Decision logic keyed on a language file extension, in either language.
report "Draft must not branch on a language file extension; use a contributed rule set:" \
  scan_rust '"(rs|py|go|ts|tsx|js|jsx|mjs|cjs|java|rb|php|cs|kt|swift)"\s*(=>|\|)'
report "The Console must not branch on a language file extension; use the contributed artifact kind:" \
  scan_web 'case\s+"(rs|py|go|ts|tsx|js|jsx|mjs|cjs|java|rb|php|cs|kt|swift)"\s*:'

# A language display table: the `display_name` of a file_association
# contribution, duplicated in a frontend.
report "The Console must not carry a language display table; read the contributed display name:" \
  scan_web '\b(rs|py|go|java|rb|php|cs|kt|swift|tsx?|jsx?)\s*:\s*"(Rust|Python|Go|Java|Ruby|PHP|C#|Kotlin|Swift|TypeScript|JavaScript)"'

# Ecosystem marker-file tables.
markers='(Cargo\.toml|Cargo\.lock|package\.json|package-lock\.json|pnpm-lock|yarn\.lock|pyproject\.toml|requirements\.txt|go\.mod|go\.sum|pom\.xml|build\.gradle|CMakeLists\.txt|Gemfile)'
report "Draft must not carry an ecosystem marker-file table; contribute it instead:" \
  scan_rust "$markers"
report "The Console must not carry an ecosystem marker-file table; contribute it instead:" \
  scan_web "$markers"

# The dead stack table this refactor removed must not come back.
if [ -e "$core_src/app/adapters.rs" ]; then
  echo "core/src/app/adapters.rs is back: stack detection is a file_association contribution." >&2
  failed=1
fi

# --- 2. The extension domain stays a leaf inside Core -----------------------

report "core/src/extension must depend only on contracts and support:" \
  rg -n --pcre2 'crate::(app|review|task|pack|trust|read_model|workspace|operation)\b' \
  "$core_src/extension" --glob '*.rs'

# --- 3. Dependency direction ------------------------------------------------

for sdk_manifest in sdk/dcg-contract/Cargo.toml sdk/extension-contract/Cargo.toml \
  sdk/draftpack-contract/Cargo.toml; do
  if [ ! -f "$root_dir/$sdk_manifest" ]; then
    echo "$sdk_manifest is missing." >&2
    failed=1
  fi
done

# The resolved graph catches every active dependency regardless of aliases or
# package names. The checker's manifest walk separately catches local path
# declarations in normal/build/dev/optional and target-specific tables that
# are inactive in the current resolution. Its self-tests exercise direct,
# aliased, and ordinary registry dependencies through the same implementation.
if ! python3 "$root_dir/scripts/check-sdk-contract-dependencies.py" --self-test; then
  failed=1
fi
# Every SDK contract crate is checked, not just the extension one: the layering
# only holds if the DCG contract is verified to be a leaf at the same time.
for sdk_crate in draft-dcg-contract draft-extension-contract draft-draftpack-contract; do
  if ! python3 "$root_dir/scripts/check-sdk-contract-dependencies.py" \
    --manifest-path "$root_dir/Cargo.toml" --contract-name "$sdk_crate"; then
    failed=1
  fi
done

# Retired spellings are assembled so the gate itself is not a stale-reference
# match. They are forbidden in active paths; historical release records would
# need a reviewed, path-specific exception here.
retired_dir="extension""-format"
retired_package="draft-extension""-format"
retired_crate="draft_extension""_format"
if [ -e "$root_dir/$retired_dir" ]; then
  echo "The retired root extension contract directory still exists." >&2
  failed=1
fi
report "Active files retain a retired extension contract name:" \
  rg -n --hidden --fixed-strings -e "$retired_package" -e "$retired_crate" -e "$retired_dir/" \
  "$root_dir" --glob '!target/**' --glob '!extensions/target/**' \
  --glob '!scripts/check-extension-architecture.sh'

if rg -q 'draft-extension-service' "$root_dir/core/Cargo.toml"; then
  echo "draft-core must not depend on draft-extension-service; the arrow points the other way." >&2
  failed=1
fi

# --- 3b. The retired file-and-diff model leaves no identifiers behind --------
#
# These are the names of the model Draft replaced: a change was a text patch, a
# resource was a file with bytes, verification selected tests, and classification
# was a single "artifact kind". Each of them silently reintroduces an assumption
# the platform is supposed to be free of, so the identifiers are banned outright
# rather than left to review.
#
# Test modules are excluded by `production_rust`: a fixture may still describe a
# domain, and often must.
retired_model_identifiers=(
  PatchSet FilePatch PatchHunk HunkOverlap
  build_text_hunks split_lines_preserve simple_unified_diff patch_graph_hash
  LsifIndex LSIF_BACKEND extract_symbols symbols_touched public_api_symbols
  scan_test_files scan_fuzz_targets run_shell
  file_association artifact_kind artifact_kinds symbol_rules verification_profile
  selected_tests selected_fuzz_targets toolchain_hash
  lsif_version test_selector_version fuzz_selector_version
  block_if_tests_fail max_changed_lines supports_patch_output
  repository_path from_working_tree file_extensions excluded_control_directories
  per_file_test full_suite toolchain_probes
  ProtectedFileAccess ProtectedFileViolation PROTECTED_FILE_ACCESS protected_files
)
retired_pattern="$(IFS='|'; echo "${retired_model_identifiers[*]}")"

retired_hits="$(production_rust "${platform_rust[@]}" \
  | rg -n --pcre2 "\\b($retired_pattern)\\b" || true)"
if [ -n "$retired_hits" ]; then
  echo "Platform code retains an identifier from the retired file-and-diff model:" >&2
  echo "$retired_hits" >&2
  failed=1
fi

retired_web_hits="$(rg -n --pcre2 "\\b($retired_pattern)\\b" \
  "$web_src" "${web_globs[@]}" || true)"
if [ -n "$retired_web_hits" ]; then
  echo "Console production source retains a retired model identifier:" >&2
  echo "$retired_web_hits" >&2
  failed=1
fi

# --- 4. New public vocabulary avoids the banned term ------------------------
#
# `provider`/`providers` are banned from the public docs by
# cli/tests/smoke.rs::docs_do_not_use_retired_external_action_terms, and code
# identifiers get quoted into those docs. Pre-existing uses are allowlisted.
# Case-insensitive and without word boundaries, so `VerificationProvider` and
# `PROVIDERS` are caught as readily as the bare word.
#
# Two exemptions, and they are different in kind.
#
# The first is expressed in the pattern: `ObservationProvider` and its
# `observation_provider` fields record *which implementation actually performed
# an observation*, which the architecture requires everywhere observations are
# handled.
#
# The second is expressed as scope. A ProviderBinding — a project's configured
# attachment to an external system — is now first-class architecture, and it is
# domain-neutral: a filesystem, a ticket tracker and a deploy target are all
# providers in that sense. Forbidding the word outright would have meant Core
# could not name a concept it is built on.
#
# But the ban was worth keeping, so it is narrowed rather than lifted. The
# vocabulary is allowed only in the files that *own* the provider model, which
# states the real architectural property: the model lives here and nowhere else.
# A subsystem reaching for the term is reaching for a concept it should be
# receiving through one of these types instead.
#
# The banned sense is unchanged: an ecosystem "provider" standing in for a
# domain Draft must not know about.
provider_owners=(
  '!**/project/provider.rs'
  # Draft's own filesystem provider: the first instance of the provider model,
  # not a subsystem reaching for the vocabulary.
  '!**/dcg/filesystem_provider.rs'
  '!**/project/provider_definition.rs'
  '!**/project/credential.rs'
  '!**/execution/plan.rs'
  '!**/project/mod.rs'
  '!**/dcg/compose.rs'
  '!**/dcg/mod.rs'
  '!**/support/lock_order.rs'
  '!**/activity/event.rs'
)
provider_globs=()
for owner in "${provider_owners[@]}"; do
  provider_globs+=(--glob "$owner")
done
report "New Draft code must not use the term 'provider'; say 'verifier', 'producer' or 'contribution source' — or, for the provider model itself, keep it in the files that own it:" \
  rg -n --pcre2 '(?i)(?<!observation)(?<!observation_)provider' \
  "$core_src" "$root_dir/sdk/extension-contract/src" "$root_dir/services/extension-service/src" \
  --glob '*.rs' "${provider_globs[@]}"

# --- 5. /extensions/ is repository-independent ------------------------------

extensions_dir="$root_dir/extensions"
if [ -d "$extensions_dir" ]; then
  # Packages are data. No code, no build files, no dependencies.
  if find "$extensions_dir/packages" -type f \
    \( -name '*.rs' -o -name '*.sh' -o -name '*.js' -o -name '*.py' -o -name 'Cargo.toml' \) \
    2>/dev/null | rg -q .; then
    echo "extensions/packages must contain declarative data only, never code or build files." >&2
    failed=1
  fi

  # Tooling and conformance tests may use the portable format crate and nothing
  # else from Draft. This is what makes the standalone move a relocation.
  # `check-standalone.sh` is excluded because it names these crates in its own
  # scan pattern; it enforces the same rule from inside `extensions/`.
  report "extensions/ may depend on draft-extension-contract only:" \
    rg -n --pcre2 '\b(draft_core|draft-core|draft_extension_service|draft-extension-service|draftd|draft_ipc|draft-ipc|draft_console|draft-console|draft_cli|draft-cli)\b' \
    "$extensions_dir" --glob '!**/target/**' --glob '!**/check-standalone.sh'

  # A package tree that reaches into a Draft store is not portable.
  # Prose that merely *names* the control plane is not reaching into it, so the
  # scan looks at package data and code rather than documentation.
  report "extensions/ must not reach into a Draft store:" \
    rg -n --fixed-strings '.draft/' "$extensions_dir" --glob '!**/target/**' --glob '!**/*.md'

  # `/extensions/` must be its own workspace, so deleting it is a no-op for the
  # platform build. Gate B proves that; this catches the cause early.
  if ! rg -q 'exclude\s*=\s*\[[^]]*"extensions"' "$root_dir/Cargo.toml"; then
    echo "The root workspace must exclude \"extensions\" so the platform never builds it." >&2
    failed=1
  fi
  if [ ! -f "$extensions_dir/Cargo.toml" ]; then
    echo "extensions/ must carry its own workspace manifest." >&2
    failed=1
  fi
fi

# --- 6. Allowlisted exceptions stay narrow, live and presentation-only ------

# An exception that no longer matches anything is an unchecked escape hatch:
# the code it excused is gone, but the hole it opened stays open. Fail on it so
# the list can only shrink by review.
while IFS= read -r entry; do
  case "$entry" in ''|'#'*) continue ;; esac
  entry_path="${entry%%:*}"
  entry_pattern="${entry#*:}"
  if [ ! -f "$root_dir/$entry_path" ]; then
    echo "Stale allowlist entry: $entry_path no longer exists." >&2
    failed=1
  elif ! rg -qF -- "$entry_pattern" "$root_dir/$entry_path"; then
    echo "Stale allowlist entry: $entry_path no longer contains '$entry_pattern'." >&2
    failed=1
  fi
done < "$allowlist"

# A renderer adapter is excused because it selects a frontend asset and decides
# nothing. If one starts doing semantic work, its allowlisted path is no longer
# a reason to let it through.
renderer="$root_dir/console/web/src/screens/project/resources/viewers/text-grammars.ts"
if [ ! -f "$renderer" ]; then
  echo "The renderer adapter moved; update this path or the check silently stops running." >&2
  failed=1
else
  # Comments are excluded deliberately: the file documents what it must not do,
  # and the rule is about the code, not the explanation of the rule.
  report "The renderer adapter must select an asset, never decide Draft semantics:" \
    rg -n --pcre2 \
    '^(?!\s*(//|\*|/\*)).*\b(capability|capabilities|verification|verifier|authoriz|permission|install|enable|disable|evidence|risk|remediation|invoke|mutation)' \
    "$renderer"
fi

if [ "$failed" -ne 0 ]; then
  echo >&2
  echo "Add a reviewed exception to scripts/core-language-allowlist.txt only when a hit is genuinely justified." >&2
  exit 1
fi

echo "Extension boundary, dependency direction, and Core domain-neutrality checks passed."
