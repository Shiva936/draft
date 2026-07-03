#!/usr/bin/env sh
set -eu

repo="${DRAFT_REPO:-Shiva936/draft}"
install_dir="${DRAFT_INSTALL_DIR:-$HOME/.local/bin}"
version_override="${DRAFT_VERSION:-}"
update_path="${DRAFT_UPDATE_PATH:-0}"

say() {
  printf '%s\n' "$*"
}

die() {
  printf 'draft install: %s\n' "$*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

download() {
  url="$1"
  dest="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$dest"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$dest" "$url"
  else
    die "curl or wget is required to download Draft"
  fi
}

json_value() {
  key="$1"
  sed -n "s/.*\"$key\"[[:space:]]*:[[:space:]]*\"\\([^\"]*\\)\".*/\\1/p" | head -n 1
}

os="$(uname -s 2>/dev/null || true)"
arch="$(uname -m 2>/dev/null || true)"

case "$os" in
  Linux)
    case "$arch" in
      x86_64|amd64) target="x86_64-unknown-linux-musl" ;;
      aarch64|arm64) target="aarch64-unknown-linux-musl" ;;
      *) die "unsupported Linux CPU architecture: $arch" ;;
    esac
    ;;
  Darwin)
    case "$arch" in
      x86_64) target="x86_64-apple-darwin" ;;
      arm64|aarch64) target="aarch64-apple-darwin" ;;
      *) die "unsupported macOS CPU architecture: $arch" ;;
    esac
    ;;
  *)
    die "unsupported operating system: $os. Use install.ps1 on native Windows PowerShell."
    ;;
esac

need_cmd tar
need_cmd mktemp

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM

if [ -n "$version_override" ]; then
  tag="v${version_override#v}"
else
  latest_json="$tmp_dir/latest.json"
  download "https://api.github.com/repos/$repo/releases/latest" "$latest_json"
  tag="$(json_value tag_name < "$latest_json")"
fi

[ -n "${tag:-}" ] || die "could not resolve latest Draft release from https://github.com/$repo"

version="${tag#v}"
asset="draft-v${version}-${target}.tar.gz"
base_url="https://github.com/$repo/releases/download/$tag"
archive="$tmp_dir/$asset"
checksums="$tmp_dir/SHA256SUMS"

say "Installing Draft $tag for $target"
download "$base_url/$asset" "$archive"
download "$base_url/SHA256SUMS" "$checksums"

expected="$(grep "  $asset\$" "$checksums" | awk '{print $1}')"
[ -n "$expected" ] || die "checksum entry for $asset was not found in SHA256SUMS"

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$tmp_dir" && printf '%s  %s\n' "$expected" "$asset" | sha256sum -c - >/dev/null)
elif command -v shasum >/dev/null 2>&1; then
  actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
  [ "$actual" = "$expected" ] || die "checksum verification failed for $asset"
else
  die "sha256sum or shasum is required to verify Draft"
fi

tar -xzf "$archive" -C "$tmp_dir"
package_dir="$tmp_dir/draft-v${version}-${target}"
[ -x "$package_dir/bin/draft" ] || die "archive did not contain bin/draft"
[ -x "$package_dir/bin/draftd" ] || die "archive did not contain bin/draftd"

mkdir -p "$install_dir" || die "could not create install directory: $install_dir"
[ -w "$install_dir" ] || die "install directory is not writable: $install_dir"

cp "$package_dir/bin/draft" "$tmp_dir/draft.new"
cp "$package_dir/bin/draftd" "$tmp_dir/draftd.new"
chmod 755 "$tmp_dir/draft.new" "$tmp_dir/draftd.new"
mv "$tmp_dir/draft.new" "$install_dir/draft"
mv "$tmp_dir/draftd.new" "$install_dir/draftd"

say "Installed draft to $install_dir/draft"
say "Installed draftd to $install_dir/draftd"

case ":$PATH:" in
  *":$install_dir:"*) ;;
  *)
    say ""
    say "$install_dir is not on PATH."
    if [ "$update_path" = "1" ] || [ "$update_path" = "true" ]; then
      profile="${DRAFT_PROFILE:-}"
      if [ -z "$profile" ]; then
        shell_name="$(basename "${SHELL:-sh}")"
        case "$shell_name" in
          zsh) profile="$HOME/.zshrc" ;;
          bash) profile="$HOME/.bashrc" ;;
          *) profile="$HOME/.profile" ;;
        esac
      fi
      touch "$profile" || die "could not update PATH profile: $profile"
      if ! grep -F "export PATH=\"$install_dir:\$PATH\"" "$profile" >/dev/null 2>&1; then
        {
          printf '\n# Draft CLI\n'
          printf 'export PATH="%s:$PATH"\n' "$install_dir"
        } >> "$profile"
      fi
      say "Updated PATH in $profile. Restart your shell or run:"
      say "  export PATH=\"$install_dir:\$PATH\""
    else
      say "Add it with:"
      say "  export PATH=\"$install_dir:\$PATH\""
      say "To let this installer update your shell profile, run with DRAFT_UPDATE_PATH=1."
    fi
    ;;
esac

if command -v "$install_dir/draft" >/dev/null 2>&1; then
  "$install_dir/draft" --version || true
fi
