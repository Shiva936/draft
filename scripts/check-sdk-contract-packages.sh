#!/usr/bin/env bash
set -euo pipefail

# Prove the exact publishable archives of the SDK contract crates are
# self-contained, and that the layering survives packaging.
#
# The SDK is a stack:
#
#     draft-extension-contract  --depends on-->  draft-dcg-contract
#     draft-dcg-contract        --depends on-->  (no Draft crate)
#
# so "the archive has no dependencies" is the wrong assertion for the upper
# crate. What is checked instead is that the DCG contract packages as a true
# leaf, and that the extension contract's only Draft dependency is the
# published DCG contract — resolved from the registry, never from a path back
# into this repository.
#
# All packaging, extraction, compilation and testing happens in fresh temporary
# directories, so nothing here can accidentally succeed by reaching the working
# tree.

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

package_target="$scratch/package-target"
verification_target="$scratch/verification-target"
mkdir -p "$package_target" "$verification_target"

# Packaging runs without --locked (see package_crate), so the working tree's
# lock file is guarded rather than trusted: a check script must not leave the
# repository different from how it found it.
lock_file="$root_dir/Cargo.lock"
lock_backup="$scratch/Cargo.lock.before"
cp "$lock_file" "$lock_backup"
restore_lock() {
  # Restore the exact bytes rather than `git checkout`, which would discard a
  # legitimate uncommitted lock change the caller meant to keep.
  if ! cmp -s "$lock_backup" "$lock_file"; then
    cp "$lock_backup" "$lock_file"
  fi
}
trap 'restore_lock; rm -rf "$scratch"' EXIT

# Packaging the dependent crate resolves its *normalized* manifest, in which
# the sibling is a registry dependency that is not published yet. A targeted
# patch lets that resolution succeed. It affects resolution only — the archive's
# contents still come from the crate directory, and verify_archive below still
# builds the extension archive against the packaged DCG archive rather than
# against this path.
export CARGO_HOME="$scratch/cargo-home"
mkdir -p "$CARGO_HOME"
cat > "$CARGO_HOME/config.toml" <<PATCH_CONFIG
[patch.crates-io]
draft-dcg-contract = { path = "$root_dir/sdk/dcg-contract" }
PATCH_CONFIG

metadata_file="$scratch/metadata.json"
CARGO_TARGET_DIR="$package_target" cargo metadata \
  --format-version 1 \
  --manifest-path "$root_dir/Cargo.toml" > "$metadata_file"

# ---------------------------------------------------------------------------
# Package each SDK crate and extract it.
# ---------------------------------------------------------------------------

declare -A extracted_root_of

package_crate() {
  local package_name="$1"
  local crate_dir="$2"

  IFS=$'\t' read -r package_id package_version target_directory license readme < <(
    python3 - "$metadata_file" "$package_name" <<'PY'
import json, sys

metadata = json.load(open(sys.argv[1]))
name = sys.argv[2]
matches = [p for p in metadata["packages"] if p["name"] == name]
if len(matches) != 1:
    raise SystemExit(f"expected one {name} package, found {len(matches)}")
package = matches[0]
print("\t".join([
    package["id"],
    package["version"],
    metadata["target_directory"],
    package.get("license") or "",
    package.get("readme") or "",
]))
PY
  )

  if [ -z "$package_id" ] || [ "$target_directory" != "$package_target" ]; then
    echo "Cargo metadata did not describe the isolated packaging environment." >&2
    exit 1
  fi
  if [ "$license" != "Apache-2.0" ] || [ -z "$readme" ]; then
    echo "$package_name must expose Apache-2.0 license metadata and a crate README." >&2
    exit 1
  fi

  local list_file="$scratch/$package_name-list.txt"
  # Two deliberate departures from a plain `cargo package`:
  #
  # `--no-verify`, because cargo's own verification builds the archive against
  # crates.io where draft-dcg-contract is not published yet. Verification is
  # moved rather than skipped: verify_archive below builds and tests each
  # archive against the *packaged* sibling, which is the stronger check — it
  # proves the archives work together, not merely that each compiles.
  #
  # No `--locked`, because packaging resolves the normalized manifest, in which
  # the sibling is a registry dependency that does not exist yet. The lock file
  # is guarded explicitly below instead, so this cannot quietly rewrite it.
  CARGO_TARGET_DIR="$package_target" cargo package \
    --manifest-path "$root_dir/Cargo.toml" \
    -p "$package_name" --allow-dirty --no-verify --list > "$list_file"
  # Two deliberate departures from a plain `cargo package`:
  #
  # `--no-verify`, because cargo's own verification builds the archive against
  # crates.io where draft-dcg-contract is not published yet. Verification is
  # moved rather than skipped: verify_archive below builds and tests each
  # archive against the *packaged* sibling, which is the stronger check — it
  # proves the archives work together, not merely that each compiles.
  #
  # No `--locked`, because packaging resolves the normalized manifest, in which
  # the sibling is a registry dependency that does not exist yet. The lock file
  # is guarded explicitly below instead, so this cannot quietly rewrite it.
  CARGO_TARGET_DIR="$package_target" cargo package \
    --manifest-path "$root_dir/Cargo.toml" \
    -p "$package_name" --allow-dirty --no-verify

  local archive="$package_target/package/$package_name-$package_version.crate"
  if [ ! -f "$archive" ]; then
    echo "Cargo did not produce the metadata-derived archive $(basename "$archive")." >&2
    exit 1
  fi

  # Every source file in the crate must actually be in the archive, or the
  # published crate is not the crate that was tested here.
  local source
  for source in "$root_dir/$crate_dir"/src/*.rs; do
    if ! grep -qF "src/$(basename "$source")" "$list_file"; then
      echo "$package_name archive is missing src/$(basename "$source")." >&2
      exit 1
    fi
  done
  for required in Cargo.toml README.md; do
    if ! grep -qF "$required" "$list_file"; then
      echo "$package_name archive is missing $required." >&2
      exit 1
    fi
  done

  local extract_dir="$scratch/extracted-$package_name"
  mkdir -p "$extract_dir"
  tar -xzf "$archive" -C "$extract_dir"
  local extracted="$extract_dir/$package_name-$package_version"
  if [ ! -f "$extracted/Cargo.toml" ]; then
    echo "$package_name archive did not extract to one crate root." >&2
    exit 1
  fi
  extracted_root_of["$package_name"]="$extracted"

  # A packaged crate that still reaches back into this repository would compile
  # here and nowhere else.
  if grep -rn --include='*.rs' -E '/extensions/|include_(str|bytes)!\s*\(\s*"(\.\./|/)' \
    "$extracted/src" 2>/dev/null; then
    echo "$package_name retains a repository-root or extensions reference." >&2
    exit 1
  fi
}

package_crate draft-dcg-contract sdk/dcg-contract
package_crate draft-extension-contract sdk/extension-contract
package_crate draft-draftpack-contract sdk/draftpack-contract

# ---------------------------------------------------------------------------
# The normalized manifests must show the layering, with no path dependencies.
# ---------------------------------------------------------------------------

if ! python3 - \
  "${extracted_root_of[draft-dcg-contract]}/Cargo.toml" \
  "${extracted_root_of[draft-extension-contract]}/Cargo.toml" \
  "${extracted_root_of[draft-draftpack-contract]}/Cargo.toml" <<'PY'
import sys, tomllib

TABLES = ("dependencies", "build-dependencies", "dev-dependencies")


def dependencies(path):
    document = tomllib.load(open(path, "rb"))
    tables = [document.get(name, {}) for name in TABLES]
    for target in document.get("target", {}).values():
        tables.extend(target.get(name, {}) for name in TABLES)
    return {name: spec for table in tables for name, spec in table.items()}


def path_dependencies(deps):
    return sorted(n for n, s in deps.items() if isinstance(s, dict) and "path" in s)


dcg_path, extension_path, draftpack_path = sys.argv[1], sys.argv[2], sys.argv[3]

dcg = dependencies(dcg_path)
if paths := path_dependencies(dcg):
    raise SystemExit(
        "draft-dcg-contract retains path dependencies after packaging: " + ", ".join(paths)
    )
if draft := sorted(n for n in dcg if n.startswith("draft")):
    raise SystemExit(
        "draft-dcg-contract must package as a leaf but depends on: " + ", ".join(draft)
    )

# Both upper crates sit directly on the DCG contract and on nothing else of
# Draft's, so the same assertion applies to each.
for name, path in (
    ("draft-extension-contract", extension_path),
    ("draft-draftpack-contract", draftpack_path),
):
    deps = dependencies(path)
    if paths := path_dependencies(deps):
        raise SystemExit(
            f"{name} retains path dependencies after packaging: " + ", ".join(paths)
        )
    draft_deps = sorted(n for n in deps if n.startswith("draft"))
    if draft_deps != ["draft-dcg-contract"]:
        raise SystemExit(
            f"{name}'s only Draft dependency must be draft-dcg-contract, found: "
            + (", ".join(draft_deps) or "none")
        )
PY
then
  exit 1
fi

# ---------------------------------------------------------------------------
# Build and test each archive outside the workspace.
#
# The extension contract's registry dependency on the DCG contract is redirected
# to the *packaged* DCG archive, so what is exercised is archive-against-archive
# rather than either one against the working tree.
# ---------------------------------------------------------------------------

verify_archive() {
  local package_name="$1"
  # The suite that must be seen to run. Compiling an archive proves it builds;
  # only running its own frozen suite proves it still behaves.
  local required_suite="$2"
  local extracted="${extracted_root_of[$package_name]}"
  local test_log="$scratch/$package_name-test.log"

  # Point the archive's registry dependency at the *packaged* DCG archive, so
  # what is exercised is archive against archive rather than either one against
  # the working tree.
  mkdir -p "$extracted/.cargo"
  cat > "$extracted/.cargo/config.toml" <<EOF
[patch.crates-io]
draft-dcg-contract = { path = "${extracted_root_of[draft-dcg-contract]}" }
EOF

  CARGO_TARGET_DIR="$verification_target" cargo check \
    --manifest-path "$extracted/Cargo.toml"
  CARGO_TARGET_DIR="$verification_target" cargo test \
    --manifest-path "$extracted/Cargo.toml" 2>&1 | tee "$test_log"

  if grep -qE '^test result: FAILED' "$test_log"; then
    echo "$package_name's packaged test suite failed outside Draft." >&2
    exit 1
  fi
  if ! grep -q "Running $required_suite" "$test_log"; then
    echo "$package_name's archive did not run $required_suite." >&2
    exit 1
  fi
}

verify_archive draft-dcg-contract 'tests/portable_closure.rs'
verify_archive draft-extension-contract 'tests/v1_compatibility.rs'
verify_archive draft-draftpack-contract 'tests/v1_format.rs'

echo "All three SDK contract archives are self-contained and preserve the SDK layering."
