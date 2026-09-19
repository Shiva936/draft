#!/usr/bin/env python3
"""Enforce the SDK contract crates' Cargo dependency boundary by package ID.

The SDK is a layered stack, not a flat set of independent crates:

    draft-extension-contract  --depends on-->  draft-dcg-contract
    draft-dcg-contract        --depends on-->  (no Draft crate)

So "reaches no workspace package" is the wrong rule: it would forbid the one
edge the architecture requires. What is actually being enforced is that each
crate reaches *only* what it is permitted to reach, and in particular that
`draft-dcg-contract` remains a leaf. A publisher takes the extension contract
and gets the DCG contract with it; nobody takes either and gets `draft-core`.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib


DEPENDENCY_TABLES = ("dependencies", "build-dependencies", "dev-dependencies")

# Which workspace packages each SDK contract crate may reach, transitively.
# An empty set means the crate must be a leaf with respect to the workspace.
ALLOWED_WORKSPACE_REACHABILITY: dict[str, set[str]] = {
    "draft-dcg-contract": set(),
    "draft-extension-contract": {"draft-dcg-contract"},
}


def resolved_workspace_reachability(metadata: dict, contract_name: str) -> list[str]:
    packages = {package["id"]: package for package in metadata["packages"]}
    workspace_members = set(metadata["workspace_members"])
    contract_ids = [
        package_id
        for package_id in workspace_members
        if packages[package_id]["name"] == contract_name
    ]
    if len(contract_ids) != 1:
        raise ValueError(
            f"expected exactly one workspace package named {contract_name!r}, found {len(contract_ids)}"
        )

    nodes = {
        node["id"]: [dependency["pkg"] for dependency in node.get("deps", [])]
        for node in metadata["resolve"]["nodes"]
    }
    contract_id = contract_ids[0]
    reachable: set[str] = set()
    pending = [contract_id]
    while pending:
        package_id = pending.pop()
        if package_id in reachable:
            continue
        reachable.add(package_id)
        pending.extend(nodes.get(package_id, []))

    return sorted(
        (reachable & workspace_members) - {contract_id},
        key=lambda package_id: (packages[package_id]["name"], package_id),
    )


def dependency_tables(document: dict):
    for table_name in DEPENDENCY_TABLES:
        yield document.get(table_name, {})
    for target in document.get("target", {}).values():
        for table_name in DEPENDENCY_TABLES:
            yield target.get(table_name, {})


def local_path_dependency_violations(
    metadata: dict, contract_name: str, workspace_manifest: Path, allowed: set[str]
) -> list[str]:
    packages = {package["id"]: package for package in metadata["packages"]}
    workspace_members = set(metadata["workspace_members"])
    workspace_manifests = {
        Path(packages[package_id]["manifest_path"]).resolve(): package_id
        for package_id in workspace_members
    }
    contract_manifests = [
        manifest
        for manifest, package_id in workspace_manifests.items()
        if packages[package_id]["name"] == contract_name
    ]
    if len(contract_manifests) != 1:
        raise ValueError(
            f"expected exactly one workspace package named {contract_name!r}, found {len(contract_manifests)}"
        )

    workspace_document = tomllib.loads(workspace_manifest.read_text())
    workspace_dependencies = workspace_document.get("workspace", {}).get("dependencies", {})
    contract_manifest = contract_manifests[0]
    violations: list[str] = []
    visited: set[Path] = set()
    pending = [contract_manifest]

    while pending:
        manifest = pending.pop()
        manifest = manifest.resolve()
        if manifest in visited:
            continue
        visited.add(manifest)
        document = tomllib.loads(manifest.read_text())
        for table in dependency_tables(document):
            for dependency_name, raw_specification in table.items():
                specification = raw_specification
                base = manifest.parent
                if isinstance(specification, dict) and specification.get("workspace") is True:
                    specification = workspace_dependencies.get(dependency_name)
                    base = workspace_manifest.parent
                if not isinstance(specification, dict) or "path" not in specification:
                    continue
                dependency_manifest = (base / specification["path"] / "Cargo.toml").resolve()
                member_id = workspace_manifests.get(dependency_manifest)
                permitted = (
                    member_id is not None and packages[member_id]["name"] in allowed
                )
                if dependency_manifest != contract_manifest and not permitted:
                    if member_id is None:
                        destination = f"repository-local manifest {dependency_manifest}"
                    else:
                        package = packages[member_id]
                        destination = (
                            f"workspace package {package['name']!r} ({member_id}) "
                            f"at {dependency_manifest}"
                        )
                    violations.append(
                        f"{manifest}: dependency {dependency_name!r} uses a local path to {destination}"
                    )
                if dependency_manifest.is_file():
                    pending.append(dependency_manifest)

    return sorted(set(violations))


def check(metadata: dict, contract_name: str, workspace_manifest: Path) -> list[str]:
    packages = {package["id"]: package for package in metadata["packages"]}
    allowed = ALLOWED_WORKSPACE_REACHABILITY.get(contract_name)
    if allowed is None:
        return [
            f"{contract_name!r} has no declared SDK reachability allowance; add one to "
            "ALLOWED_WORKSPACE_REACHABILITY rather than widening the check"
        ]
    failures = []
    for package_id in resolved_workspace_reachability(metadata, contract_name):
        package = packages[package_id]
        if package["name"] in allowed:
            continue
        failures.append(
            f"resolved dependency graph reaches workspace package {package['name']!r} "
            f"({package_id}) at {package['manifest_path']}"
        )
    failures.extend(
        local_path_dependency_violations(metadata, contract_name, workspace_manifest, allowed)
    )
    return failures


def synthetic_metadata(
    root: Path,
    contract_name: str,
    dependency_id: str,
    dependency_name: str,
    dependency_in_workspace: bool,
) -> dict:
    contract_manifest = root / "contract" / "Cargo.toml"
    dependency_manifest = root / "dependency" / "Cargo.toml"
    contract_id = f"path+file:///contract#{contract_name}@0.3.4"
    dependency = {
        "id": dependency_id,
        "name": dependency_name,
        "manifest_path": str(dependency_manifest),
    }
    if not dependency_in_workspace:
        dependency["source"] = "registry"
    return {
        "packages": [
            {
                "id": contract_id,
                "name": contract_name,
                "manifest_path": str(contract_manifest),
            },
            dependency,
        ],
        "workspace_members": [contract_id]
        + ([dependency_id] if dependency_in_workspace else []),
        "resolve": {
            "nodes": [
                {"id": contract_id, "deps": [{"pkg": dependency_id}]},
                {"id": dependency_id, "deps": []},
            ]
        },
    }


def self_test() -> None:
    """Prove the checker rejects what it claims to, and permits the SDK edge.

    A checker that silently passed everything would look identical to a healthy
    dependency graph, so each rule is exercised against a synthetic graph built
    to violate exactly that rule.
    """
    with tempfile.TemporaryDirectory(prefix="draft-contract-graph-") as directory:
        root = Path(directory)
        (root / "contract").mkdir()
        (root / "dependency").mkdir()
        workspace_manifest = root / "Cargo.toml"
        workspace_manifest.write_text(
            "[workspace]\nmembers = [\"contract\", \"dependency\"]\n"
        )

        def write_contract(name: str, dependency_line: str) -> None:
            (root / "contract" / "Cargo.toml").write_text(
                f"[package]\nname = \"{name}\"\nversion = \"0.3.4\"\n"
                f"[dependencies]\n{dependency_line}\n"
            )

        def write_dependency(name: str) -> None:
            (root / "dependency" / "Cargo.toml").write_text(
                f"[package]\nname = \"{name}\"\nversion = \"0.3.4\"\n"
            )

        # 1. Reaching draft-core is forbidden for every SDK crate, whether the
        #    dependency is named directly or hidden behind an alias.
        write_dependency("draft-core")
        core_id = "path+file:///dependency#draft-core@0.3.4"
        for contract_name in ALLOWED_WORKSPACE_REACHABILITY:
            for label, dependency_line in (
                ("direct", 'draft-core = { path = "../dependency" }'),
                ("aliased", 'internal = { package = "draft-core", path = "../dependency" }'),
            ):
                write_contract(contract_name, dependency_line)
                metadata = synthetic_metadata(
                    root, contract_name, core_id, "draft-core", True
                )
                if not check(metadata, contract_name, workspace_manifest):
                    raise AssertionError(
                        f"{contract_name}: {label} draft-core dependency was not rejected"
                    )

        # 2. The layered SDK edge is permitted, by path and by resolved graph.
        write_dependency("draft-dcg-contract")
        dcg_id = "path+file:///dependency#draft-dcg-contract@0.3.4"
        write_contract(
            "draft-extension-contract",
            'draft-dcg-contract = { version = "0.3.4", path = "../dependency" }',
        )
        metadata = synthetic_metadata(
            root, "draft-extension-contract", dcg_id, "draft-dcg-contract", True
        )
        if check(metadata, "draft-extension-contract", workspace_manifest):
            raise AssertionError("the permitted extension-contract -> dcg-contract edge was rejected")

        # 3. The same edge is forbidden in reverse: the DCG contract is a leaf.
        write_contract(
            "draft-dcg-contract",
            'draft-extension-contract = { version = "0.3.4", path = "../dependency" }',
        )
        write_dependency("draft-extension-contract")
        extension_id = "path+file:///dependency#draft-extension-contract@0.3.4"
        metadata = synthetic_metadata(
            root, "draft-dcg-contract", extension_id, "draft-extension-contract", True
        )
        if not check(metadata, "draft-dcg-contract", workspace_manifest):
            raise AssertionError("draft-dcg-contract was allowed to depend on the extension contract")

        # 4. An SDK crate with no declared allowance fails closed rather than
        #    being waved through.
        write_contract("draft-unknown-contract", 'serde = "1"')
        registry_id = "registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0"
        metadata = synthetic_metadata(
            root, "draft-unknown-contract", registry_id, "serde", False
        )
        if not check(metadata, "draft-unknown-contract", workspace_manifest):
            raise AssertionError("an undeclared SDK crate was not rejected")

        # 5. An ordinary registry dependency is fine.
        write_contract("draft-dcg-contract", 'serde = "1"')
        metadata = synthetic_metadata(
            root, "draft-dcg-contract", registry_id, "serde", False
        )
        if check(metadata, "draft-dcg-contract", workspace_manifest):
            raise AssertionError("a normal registry dependency was rejected")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest-path", type=Path)
    parser.add_argument("--contract-name", default="draft-extension-contract")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        print("SDK contract dependency checker self-tests passed.")
        return 0
    if args.manifest_path is None:
        parser.error("--manifest-path is required unless --self-test is used")

    manifest = args.manifest_path.resolve()
    environment = os.environ.copy()
    result = subprocess.run(
        [
            "cargo",
            "metadata",
            "--format-version",
            "1",
            "--manifest-path",
            str(manifest),
        ],
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        env=environment,
    )
    metadata = json.loads(result.stdout)
    failures = check(metadata, args.contract_name, manifest)
    if failures:
        print(
            f"{args.contract_name} crosses its permitted SDK dependency boundary:",
            file=sys.stderr,
        )
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1
    print(f"{args.contract_name} Cargo dependency graph respects the SDK layering.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
