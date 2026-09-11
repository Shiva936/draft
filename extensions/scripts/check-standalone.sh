#!/usr/bin/env bash
set -euo pipefail

# Gate C — `/extensions/` stands on its own.
#
# Everything here runs from inside `extensions/`, using only its own workspace
# and the published `draft-extension-contract` crate. Nothing reaches into Draft's
# private source, and nothing needs a Draft build, a daemon or a store.
#
# This is the rehearsal for the move: when these sources become the
# `draft-extensions` repository, this script is what still has to pass, and the
# only thing that changes is where `draft-extension-contract` comes from.

extensions_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$extensions_dir"

echo "--- no Draft-internal dependency ---"
# The format crate is the whole permitted surface. Anything else would make the
# standalone move an implementation rewrite instead of a relocation.
if rg -n --pcre2 '\b(draft_core|draft-core|draft_extension_service|draft-extension-service|draftd|draft_ipc|draft-ipc|draft_console|draft-console|draft_cli|draft-cli)\b' \
  . --glob '!target/**' --glob '!scripts/check-standalone.sh'; then
  echo "extensions/ may depend on draft-extension-contract only." >&2
  exit 1
fi

echo "--- packages are data, not code ---"
if find packages -type f \( -name '*.rs' -o -name '*.sh' -o -name '*.js' -o -name '*.py' -o -name 'Cargo.toml' \) | rg -q .; then
  echo "extensions/packages must contain declarative data only." >&2
  exit 1
fi

echo "--- format conformance ---"
cargo test --locked

echo "--- the packaging tool builds ---"
cargo build --locked --bin draft-extension-packager

out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT
packager() { cargo run --locked --quiet --bin draft-extension-packager -- "$@"; }

echo "--- validate ---"
packager validate packages

echo "--- package and derive catalog metadata ---"
packager build packages "$out" --catalog-id draft-official

echo "--- sign with an ephemeral CI key ---"
# Created here, used once, and gone with the scratch directory. The packager
# never generates or stores key material of its own, so CI supplies this the
# same way an authorized signing environment would supply a real one.
python3 - "$out/ci.key" <<'PY'
import base64, sys, os
sys.argv[1]
with open(sys.argv[1], "w") as handle:
    handle.write(base64.b64encode(os.urandom(32)).decode())
PY
packager sign "$out" --key "$out/ci.key" --key-id ci-ephemeral
rm -f "$out/ci.key"

echo "--- verify the signed catalog ---"
packager verify "$out"

echo "--- catalog metadata matches the package manifests ---"
python3 - "$out" "$extensions_dir/packages" <<'PY'
import json, pathlib, sys

catalog_dir, packages_dir = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
targets = json.loads((catalog_dir / "targets.json").read_text())["signed"]["packages"]

# A capability *is* the contribution kind it comes from. Keeping a translation
# table here would let the two drift silently, and the whole point of this check
# is that the catalog says exactly what the manifests say.
failures = []
for target in targets:
    manifest = json.loads((packages_dir / target["id"] / "extension.json").read_text())
    expected_capabilities = sorted({c["kind"] for c in manifest["contributions"]})
    for field, mine, theirs in [
        ("name", target.get("name"), manifest.get("name")),
        ("description", target.get("description"), manifest.get("description")),
        ("keywords", target.get("keywords", []), manifest.get("keywords", [])),
        ("capabilities", target.get("capabilities", []), expected_capabilities),
        ("version", target["version"], manifest["version"]),
        ("publisher", target["publisher"], manifest["publisher"]),
    ]:
        if mine != theirs:
            failures.append(f"{target['id']}: {field} is {mine!r} in the catalog but {theirs!r} in the manifest")

if failures:
    print("\n".join(failures), file=sys.stderr)
    sys.exit(1)
print(f"{len(targets)} target(s) match their manifests")
PY

echo "--- no key material is left behind ---"
if find . -type f \( -name '*.pem' -o -name '*.key' -o -name 'id_ed25519*' \) -not -path './target/*' | rg -q .; then
  echo "signing key material must never be written into the extension sources." >&2
  exit 1
fi

echo
echo "/extensions/ validates, packages, signs and verifies using only the public Draft boundary."
