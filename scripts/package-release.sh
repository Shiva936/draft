#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
  printf 'usage: %s <version> <target-triple> [dist-dir]\n' "$0" >&2
  exit 2
fi

version="${1#v}"
target="$2"
dist_dir="${3:-dist}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
release_dir="$repo_root/target/$target/release"

case "$dist_dir" in
  /*) output_dir="$dist_dir" ;;
  *) output_dir="$repo_root/$dist_dir" ;;
esac

case "$target" in
  x86_64-pc-windows-msvc)
    archive="draft-v${version}-${target}.zip"
    draft_bin="draft.exe"
    draftd_bin="draftd.exe"
    ;;
  *)
    archive="draft-v${version}-${target}.tar.gz"
    draft_bin="draft"
    draftd_bin="draftd"
    ;;
esac

if [ ! -x "$release_dir/$draft_bin" ] && [ ! -f "$release_dir/$draft_bin" ]; then
  printf 'missing built binary: %s\n' "$release_dir/$draft_bin" >&2
  exit 1
fi

if [ ! -x "$release_dir/$draftd_bin" ] && [ ! -f "$release_dir/$draftd_bin" ]; then
  printf 'missing built binary: %s\n' "$release_dir/$draftd_bin" >&2
  exit 1
fi

package_name="draft-v${version}-${target}"
work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT

mkdir -p "$work_dir/$package_name/bin" "$output_dir"
cp "$release_dir/$draft_bin" "$work_dir/$package_name/bin/"
cp "$release_dir/$draftd_bin" "$work_dir/$package_name/bin/"
cp "$repo_root/README.md" "$work_dir/$package_name/"
cp "$repo_root/LICENSE" "$work_dir/$package_name/"

if [ -f "$repo_root/NOTICE" ]; then
  cp "$repo_root/NOTICE" "$work_dir/$package_name/"
fi

(
  cd "$work_dir"
  case "$archive" in
    *.zip)
      if command -v zip >/dev/null 2>&1; then
        zip -qr "$output_dir/$archive" "$package_name"
      elif command -v powershell >/dev/null 2>&1; then
        ARCHIVE_PATH="$output_dir/$archive" PACKAGE_NAME="$package_name" powershell -NoProfile -Command 'Compress-Archive -Path $env:PACKAGE_NAME -DestinationPath $env:ARCHIVE_PATH -Force'
      elif command -v pwsh >/dev/null 2>&1; then
        ARCHIVE_PATH="$output_dir/$archive" PACKAGE_NAME="$package_name" pwsh -NoProfile -Command 'Compress-Archive -Path $env:PACKAGE_NAME -DestinationPath $env:ARCHIVE_PATH -Force'
      else
        printf 'zip, powershell, or pwsh is required to create %s\n' "$archive" >&2
        exit 1
      fi
      ;;
    *.tar.gz)
      tar -czf "$output_dir/$archive" "$package_name"
      ;;
  esac
)

printf '%s\n' "$output_dir/$archive"
