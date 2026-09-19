#!/usr/bin/env bash
set -euo pipefail

# Prove `draft-dcg-contract` is transitively dependency-closed and Core-free.
#
# Documentation cannot establish this. A canonical type could acquire a
# Core-owned field at any depth and every test inside the workspace would still
# pass, because the workspace makes `draft-core` available. So this gate builds
# a scratch crate OUTSIDE the workspace whose only dependency is the SDK crate,
# runs the portable closure suite there, and inspects the resolved dependency
# graph.
#
# The cheap, precise checks run before the expensive build on purpose: a
# forbidden dependency should be reported by the check written to name it, not
# as an incidental compile failure three minutes later.

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
crate_dir="$root_dir/sdk/dcg-contract"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

# ---------------------------------------------------------------------------
# 1. The dependency set is an allowlist.
#
#    Grepping sources for library names is too weak — it misses an aliased
#    import and trips over prose. What actually constrains the crate is its
#    manifest, so that is what is checked: any dependency outside the frozen
#    dependency-light set is a leak, whether or not it is used yet.
# ---------------------------------------------------------------------------

if ! python3 - "$crate_dir/Cargo.toml" <<'MANIFEST_PY'
import sys, tomllib

ALLOWED = {"serde", "serde_json", "ed25519-dalek", "base64", "sha2"}

document = tomllib.load(open(sys.argv[1], "rb"))
tables = []
for name in ("dependencies", "build-dependencies", "dev-dependencies"):
    tables.append(document.get(name, {}))
for target in document.get("target", {}).values():
    for name in ("dependencies", "build-dependencies", "dev-dependencies"):
        tables.append(target.get(name, {}))

declared = {name for table in tables for name in table}
unexpected = sorted(declared - ALLOWED)
if unexpected:
    raise SystemExit(
        "draft-dcg-contract declares dependencies outside the dependency-light "
        "allowlist: " + ", ".join(unexpected)
    )

paths = [
    name
    for table in tables
    for name, spec in table.items()
    if isinstance(spec, dict) and "path" in spec
]
if paths:
    raise SystemExit(
        "draft-dcg-contract must depend on no Draft crate, but declares path "
        "dependencies: " + ", ".join(paths)
    )
MANIFEST_PY
then
  exit 1
fi

# Provider runtime and secret indirection must not be imported. Naming either
# in a comment to explain why it is absent is correct and is not a leak, so
# only real `use` statements count.
if hits="$(grep -rnE '^\s*(pub )?use +(reqwest|hyper|git2|tokio|ureq|rusoto|aws_sdk|octocrab)\b' \
  "$crate_dir/src" 2>/dev/null)"; then
  echo "Provider or network runtime is imported by the portable DCG contract:" >&2
  echo "$hits" >&2
  exit 1
fi
if hits="$(grep -rnE '\bCredentialHandleRef\b' "$crate_dir/src" 2>/dev/null | grep -vE ':[0-9]+:\s*//')"; then
  echo "CredentialHandleRef is operational and must not appear in the portable contract:" >&2
  echo "$hits" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# 2. One canonical value, one Rust type.
#
#    A duplicate definition in Core would satisfy every other check here while
#    letting the two definitions' canonical meaning drift apart, which is
#    exactly the failure single ownership exists to prevent.
# ---------------------------------------------------------------------------

sdk_owned=(
  SecurityFactRef
  SecurityControlKindId
  LeaseId
  LeaseFence
  ProjectSecurityStateDigest
  PolicyDigest
  ProjectControlGeneration
  ProviderBindingGeneration
  CredentialAuthorityClass
  AuthorityDecision
  ProviderProvenanceRef
  ProviderRouteRef
  ObservationRef
  ObservationRunRef
  PublicationRef
  PublicationAttemptRef
)
search_roots=("$root_dir/core/src")
for service in "$root_dir"/services/*/src; do
  [ -d "$service" ] && search_roots+=("$service")
done

for type_name in "${sdk_owned[@]}"; do
  if hits="$(grep -rnE "^\s*(pub )?(struct|enum) $type_name\b" "${search_roots[@]}" 2>/dev/null)"; then
    echo "Canonical type $type_name is owned by draft-dcg-contract but redefined in Core:" >&2
    echo "$hits" >&2
    exit 1
  fi
done

# ---------------------------------------------------------------------------
# 3. The scratch crate: only draft-dcg-contract, deliberately detached from the
#    Draft workspace so nothing else can be resolved implicitly.
# ---------------------------------------------------------------------------

scratch_crate="$scratch/portable-verifier"
mkdir -p "$scratch_crate/src" "$scratch_crate/tests"

cat > "$scratch_crate/Cargo.toml" <<EOF
[workspace]

[package]
name = "portable-verifier"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
draft-dcg-contract = { path = "$crate_dir" }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
EOF

cat > "$scratch_crate/src/lib.rs" <<'EOF'
//! An independent verifier holding only the portable DCG contract.
EOF

cp "$crate_dir/tests/portable_closure.rs" "$scratch_crate/tests/portable_closure.rs"

test_log="$scratch/closure-test.log"
CARGO_TARGET_DIR="$scratch/target" cargo test \
  --manifest-path "$scratch_crate/Cargo.toml" 2>&1 | tee "$test_log"

if ! grep -q 'Running tests/portable_closure\.rs' "$test_log"; then
  echo "The portable closure suite did not execute outside the workspace." >&2
  exit 1
fi
if grep -qE '^test result: FAILED' "$test_log"; then
  echo "The portable closure suite failed outside the workspace." >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# 4. The resolved graph must contain no Draft crate but the SDK itself.
# ---------------------------------------------------------------------------

tree_output="$scratch/tree.txt"
CARGO_TARGET_DIR="$scratch/target" cargo tree \
  --manifest-path "$scratch_crate/Cargo.toml" \
  --edges normal,build > "$tree_output"

forbidden=(
  draft-core
  draft-extension-contract
  draft-extension-service
  draftd
  draft-cli
  draft-console
  draft-console-application
  draft-console-tui
  draft-ipc
  draft-store
  draft-locks
  draft-sessions
  draft-workspaces
  draft-sync
  draft-watcher
)
for crate in "${forbidden[@]}"; do
  if grep -qE "(^|[^a-z-])$crate v" "$tree_output"; then
    echo "draft-dcg-contract transitively depends on $crate:" >&2
    grep -nE "(^|[^a-z-])$crate v" "$tree_output" >&2
    exit 1
  fi
done

# The SDK crate itself must of course be there, or the graph proves nothing.
if ! grep -qE 'draft-dcg-contract v' "$tree_output"; then
  echo "The scratch crate did not resolve draft-dcg-contract at all." >&2
  exit 1
fi

echo "draft-dcg-contract is dependency-closed, Core-free, and singly owned."
