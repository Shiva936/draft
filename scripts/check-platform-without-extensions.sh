#!/usr/bin/env bash
set -euo pipefail

# Gate B — Draft is a complete platform with `/extensions/` physically absent.
#
# `/extensions/` is its own workspace and the root manifest excludes it, so its
# absence should be a genuine no-op. This proves it rather than assuming it:
# the repository is copied to a scratch directory, `extensions/` is deleted
# outright, and the platform is built and tested there.
#
# A build that quietly depends on the extension sources — a path dependency, an
# `include_str!`, a test fixture reaching sideways — fails here.

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

copy="$scratch/draft"
mkdir -p "$copy"

# Copy the repository without the artifacts that would make this slow or
# meaningless. `git ls-files` would miss the uncommitted work under review, so
# copy the tree and prune build output instead.
tar --create --directory "$root_dir" \
  --exclude='./target' \
  --exclude='./extensions/target' \
  --exclude='./fuzz/target' \
  --exclude='./console/web/node_modules' \
  --exclude='./.git' \
  . | tar --extract --directory "$copy"

if [ ! -d "$copy/extensions" ]; then
  echo "The copy has no extensions/ directory, so this gate would prove nothing." >&2
  exit 1
fi
rm -rf "$copy/extensions"

cd "$copy"

echo "--- the platform compiles without /extensions/ ---"
cargo check --workspace --all-targets --locked

echo "--- its tests pass without /extensions/ ---"
# The CLI smoke suite is excluded here only because it is minutes long and Gate
# A already runs it in full; everything that could reach into extensions/ —
# core, the extension service, the daemon, the Console — runs.
cargo test --locked \
  -p draft-extension-contract \
  -p draft-core \
  -p draft-ipc \
  -p draft-store \
  -p draft-locks \
  -p draft-sessions \
  -p draft-sync \
  -p draft-watcher \
  -p draft-console \
  -p draft-console-application \
  -p draft-console-tui \
  --all-targets

echo "--- the surface inventories still hold ---"
cargo test --locked -p draft-cli --test surface
cargo test --locked -p draftd --test method_surface

echo "--- the extension service works, minus the tests that need the packages ---"
# `official_packages` is the one suite that legitimately needs `/extensions/`;
# it is Gate C's counterpart and runs in Gate A.
cargo test --locked -p draft-extension-service --lib
cargo test --locked -p draft-extension-service --test authorization

echo
echo "Draft builds, tests and keeps its full surface with /extensions/ absent."
