#!/usr/bin/env python3
"""Release-time manifest tooling for `draft update` trust (stage 2).

  generate          Build dist/release-manifest.json. `trusted_key_ids` is taken
                    from the packaged binaries themselves: each build job ran its
                    own packaged `draft release-trust-set --json` and saved
                    `draft-v<version>-<target>.trust-set.json`. All targets must
                    report the identical ordered list (ReleaseTrustSetMismatch);
                    the list is never derived from secret names or config.
  retirement-gate   Refuse a release that stops carrying a previously active
                    key's signature unless a still-published STABLE bridge,
                    signed by that key, declares a currently active key
                    (ReleaseRetirementBridgeInvalid).
  check             Self-check: every artifact in dist matches the manifest, and
                    every signature file names a key the manifest declares.

Signing itself is done with openssl in the workflow over the exact bytes this
script wrote; this script never sees private key material.
"""
from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import pathlib
import sys
import urllib.request

TARGETS = [
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
]


def fail(code: str, message: str) -> None:
    print(f"{code}: {message}", file=sys.stderr)
    sys.exit(1)


def artifact_name(version: str, target: str) -> str:
    extension = "zip" if "windows" in target else "tar.gz"
    return f"draft-v{version}-{target}.{extension}"


def check_trust_sets(sets: dict[str, list[str]]) -> list[str]:
    """Exact ordered equality across every target (I53)."""
    if not sets:
        fail("ReleaseTrustSetMismatch", "no packaged trust sets were produced")
    first_target, first = next(iter(sets.items()))
    if len(set(first)) != len(first):
        fail("ReleaseTrustSetMismatch", f"{first_target} lists a key id twice: {first}")
    for target, ids in sets.items():
        if ids != first:
            fail("ReleaseTrustSetMismatch", f"{target} embeds {ids}, {first_target} embeds {first}")
    return first


def generate(args: argparse.Namespace) -> None:
    dist = pathlib.Path(args.dist)
    sets = {}
    for target in TARGETS:
        path = dist / f"draft-v{args.version}-{target}.trust-set.json"
        if not path.is_file():
            fail("ReleaseTrustSetMismatch", f"{target} produced no trust-set sidecar")
        sets[target] = json.loads(path.read_text())
    trusted = check_trust_sets(sets)
    if not trusted and not args.allow_empty:
        fail("ReleaseTrustSetMismatch",
             "the binaries embed no release-verification key (RELEASE_TRUSTED_KEYS is empty)")
    artifacts = []
    for target in TARGETS:
        asset = artifact_name(args.version, target)
        data = (dist / asset).read_bytes()
        artifacts.append({
            "target": target,
            "asset": asset,
            "sha256": hashlib.sha256(data).hexdigest(),
            "size": len(data),
        })
    manifest = {
        "schema_version": 1,
        "version": args.version,
        "channel": "prerelease" if "-" in args.version else "stable",
        "tag": f"v{args.version}",
        "trusted_key_ids": trusted,
        "artifacts": artifacts,
        "generated_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    }
    (dist / "release-manifest.json").write_bytes(json.dumps(manifest, indent=2).encode() + b"\n")
    for target in TARGETS:
        (dist / f"draft-v{args.version}-{target}.trust-set.json").unlink()
    print(f"release-manifest.json declares {trusted}")


def github(url: str):
    request = urllib.request.Request(url, headers={
        "Accept": "application/vnd.github+json",
        "User-Agent": "draft-release",
        **({"Authorization": f"Bearer {os.environ['GH_TOKEN']}"} if os.environ.get("GH_TOKEN") else {}),
    })
    with urllib.request.urlopen(request, timeout=20) as response:
        return json.loads(response.read())


def published_releases(repo: str) -> list[dict]:
    releases = []
    for page in range(1, 11):
        batch = github(f"https://api.github.com/repos/{repo}/releases?per_page=100&page={page}")
        releases.extend(r for r in batch if not r.get("draft") and r.get("published_at"))
        if len(batch) < 100:
            return releases
    fail("ReleaseRetirementBridgeInvalid", "the release history exceeds the scan bound")
    return releases


def signers(release: dict) -> set[str]:
    names = [asset["name"] for asset in release.get("assets", [])]
    return {name[len("release-manifest."):-len(".sig")]
            for name in names
            if name.startswith("release-manifest.rk_") and name.endswith(".sig")}


def manifest_of(repo: str, release: dict) -> dict | None:
    for asset in release.get("assets", []):
        if asset["name"] == "release-manifest.json":
            request = urllib.request.Request(asset["browser_download_url"], headers={"User-Agent": "draft-release"})
            with urllib.request.urlopen(request, timeout=20) as response:
                return json.loads(response.read())
    return None


def retirement_gate(args: argparse.Namespace) -> None:
    active = [key for key in args.active_key_ids.split(",") if key]
    releases = published_releases(args.repo)
    if not releases:
        print("no published releases: nothing is being retired")
        return
    newest = max(releases, key=lambda r: r["published_at"])
    retiring = signers(newest) - set(active)
    for key in sorted(retiring):
        bridged = False
        for release in releases:
            if release.get("prerelease") or key not in signers(release):
                continue
            manifest = manifest_of(args.repo, release) or {}
            if manifest.get("channel") == "stable" and set(manifest.get("trusted_key_ids", [])) & set(active):
                bridged = True
                break
        if not bridged:
            fail("ReleaseRetirementBridgeInvalid",
                 f"{key} would stop signing, but no published stable release signed by {key} "
                 f"declares an active key {active}")
    print(f"retirement gate passed (retiring: {sorted(retiring) or 'none'})")


def check(args: argparse.Namespace) -> None:
    dist = pathlib.Path(args.dist)
    manifest = json.loads((dist / "release-manifest.json").read_bytes())
    for artifact in manifest["artifacts"]:
        data = (dist / artifact["asset"]).read_bytes()
        if hashlib.sha256(data).hexdigest() != artifact["sha256"] or len(data) != artifact["size"]:
            fail("ReleaseMetadataInvalid", f"{artifact['asset']} does not match the manifest")
    declared = set(manifest["trusted_key_ids"])
    for signature in dist.glob("release-manifest*.sig"):
        envelope = json.loads(signature.read_text())
        if envelope["key_id"] not in declared:
            fail("ReleaseSignatureInvalid", f"{signature.name} is signed by an undeclared key")
        if signature.name != "release-manifest.sig" and signature.name != f"release-manifest.{envelope['key_id']}.sig":
            fail("ReleaseSignatureInvalid", f"{signature.name} names {envelope['key_id']}")
    print("manifest, artifacts and signatures are consistent")


def self_test() -> None:
    check_trust_sets({"a": ["A", "B"], "b": ["A", "B"]})
    for bad in [{"a": ["A", "B"], "b": ["A"]}, {"a": ["A"], "b": ["A", "B"]},
                {"a": ["A", "B"], "b": ["B", "A"]}, {"a": ["A", "A"]}]:
        try:
            check_trust_sets(bad)
        except SystemExit:
            continue
        raise AssertionError(f"accepted {bad}")
    print("self-test passed")


def main() -> None:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    g = sub.add_parser("generate")
    g.add_argument("--version", required=True)
    g.add_argument("--dist", default="dist")
    g.add_argument("--allow-empty", action="store_true")
    r = sub.add_parser("retirement-gate")
    r.add_argument("--repo", required=True)
    r.add_argument("--active-key-ids", required=True)
    c = sub.add_parser("check")
    c.add_argument("--dist", default="dist")
    sub.add_parser("self-test")
    args = parser.parse_args()
    {"generate": generate, "retirement-gate": retirement_gate, "check": check,
     "self-test": lambda _: self_test()}[args.command](args)


if __name__ == "__main__":
    main()
