#!/usr/bin/env sh
# Draft installer (Linux, macOS, WSL).
#
# Trust: this installer verifies the downloaded archive over HTTPS against the
# release's published SHA256SUMS (stage 1). It does not verify the signed
# release manifest; that stage-2 trust applies to `draft update` once Draft
# is installed.
#
# Mutation: this script never writes the installation. It routes, downloads,
# verifies and extracts, then launches a Rust lifecycle actor, which takes
# <install_root>/.draft-install/lifecycle.lock, re-reads the state under it,
# and performs every change.
set -eu

repo="${DRAFT_REPO:-Shiva936/draft}"
path_bin="${DRAFT_INSTALL_DIR:-$HOME/.local/bin}"
install_root="${DRAFT_INSTALL_ROOT:-$HOME/.local/share/draft}"
version_override="${DRAFT_VERSION:-}"
update_path="${DRAFT_UPDATE_PATH:-0}"
migrate_legacy="${DRAFT_MIGRATE_LEGACY_PATH:-0}"

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
    curl -fsSL --proto '=https' "$url" -o "$dest"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$dest" "$url"
  else
    die "curl or wget is required to download Draft"
  fi
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    die "sha256sum or shasum is required to verify Draft"
  fi
}

json_value() {
  key="$1"
  sed -n "s/.*\"$key\"[[:space:]]*:[[:space:]]*\"\\([^\"]*\\)\".*/\\1/p" | head -n 1
}

case "$install_root" in
  /*) ;;
  *) die "DRAFT_INSTALL_ROOT must be an absolute path: $install_root" ;;
esac
case "$path_bin" in
  /*) ;;
  *) die "DRAFT_INSTALL_DIR must be an absolute path: $path_bin" ;;
esac

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

lifecycle="$install_root/.draft-install"
installed_draft="$install_root/bin/draft"

# --- Lifecycle preflight --------------------------------------------------
# Fixed-slot existence and the bounded bootstrap.recovery grammar only. This
# never parses operation.json, never infers an operation kind, and grants no
# mutation authority: the Rust actor reclassifies under the lock.

h='[0-9a-f]'
h12="$h$h$h$h$h$h$h$h$h$h$h$h"
h64="$h12$h12$h12$h12$h12$h$h$h$h"
boot_installation=""
boot_operation=""
boot_sha256=""
boot_size=""

# Validate bootstrap.recovery against its closed six-line grammar. Sets the
# boot_* values on success; returns non-zero on any violation.
read_bootstrap() {
  record="$1"
  size="$(wc -c < "$record" | tr -d ' ')"
  [ "$size" -le 512 ] || return 1
  n=0
  cr="$(printf '\r')"
  while IFS= read -r line || { [ -n "$line" ] && return 1; }; do
    line="${line%"$cr"}"
    [ "${#line}" -le 128 ] || return 1
    case "$line" in
      *[![:print:]]*) return 1 ;;
    esac
    n=$((n + 1))
    case "$n:$line" in
      "1:draft-lifecycle-bootstrap 1") ;;
      2:"installation ins_"$h12) boot_installation="${line#installation }" ;;
      3:"operation ilo_"$h12) boot_operation="${line#operation }" ;;
      "4:kind uninstall") ;;
      5:"helper-sha256 "$h64) boot_sha256="${line#helper-sha256 }" ;;
      6:"helper-size "*)
        boot_size="${line#helper-size }"
        case "$boot_size" in
          ''|*[!0-9]*) return 1 ;;
        esac
        ;;
      *) return 1 ;;
    esac
  done < "$record"
  [ "$n" -eq 6 ]
}

route() {
  journal=0
  [ -f "$lifecycle/operation.json" ] && journal=1
  bootstrap=0
  [ -f "$lifecycle/bootstrap.recovery" ] && bootstrap=1
  if [ "$journal" -eq 1 ] && [ "$bootstrap" -eq 1 ]; then
    echo A
  elif [ "$journal" -eq 1 ] && [ -x "$installed_draft" ]; then
    echo B
  elif [ "$journal" -eq 1 ]; then
    echo C
  elif [ "$bootstrap" -eq 1 ] && [ ! -f "$lifecycle/terminal-cleanup" ]; then
    echo E
  else
    # D (terminal residue) and F (nothing, or the inert skeleton) are both
    # handed to the coordinator, which consumes READY first under the lock.
    echo F
  fi
}

unavailable() {
  printf 'draft install: an unfinished uninstall at %s cannot continue: %s\n' "$install_root" "$1" >&2
  [ -n "$boot_installation" ] && printf '  installation %s, operation %s\n' "$boot_installation" "$boot_operation" >&2
  [ -n "$boot_operation" ] && printf '  helper slot: %s/staging/%s/draft (expected sha256 %s, size %s)\n' \
    "$lifecycle" "$boot_operation" "$boot_sha256" "$boot_size" >&2
  cat >&2 <<'EOF'
  This lifecycle state requires manual lifecycle repair/support because the
  operation's previously authorized executor is unavailable.
  Do NOT delete lifecycle.lock; do NOT delete operation.json; do NOT
  recursively remove .draft-install/; do NOT delete the installation root.
EOF
  exit 1
}

recover_uninstall() {
  read_bootstrap "$lifecycle/bootstrap.recovery" || {
    boot_installation=""
    boot_operation=""
    unavailable "bootstrap.recovery is malformed"
  }
  helper="$lifecycle/staging/$boot_operation/draft"
  if [ ! -f "$helper" ] || [ "$(sha256_of "$helper")" != "$boot_sha256" ] \
    || [ "$(wc -c < "$helper" | tr -d ' ')" != "$boot_size" ]; then
    # A still-present installed Draft may re-stage the helper itself.
    if [ -x "$installed_draft" ]; then
      "$installed_draft" __installer recover || unavailable "the installed Draft could not resume it"
      return
    fi
    if [ -f "$helper" ]; then
      unavailable "the staged helper does not match its recorded identity"
    fi
    unavailable "the staged helper is missing and no installed Draft remains"
  fi
  # Identity/integrity consistency inside the installation's private
  # lifecycle directory — not a signature. The helper validates everything
  # authoritatively against operation.json before acting.
  say "Resuming an interrupted uninstall at $install_root"
  "$helper" __lifecycle-helper --installation-id "$boot_installation" \
    --operation-id "$boot_operation" --bootstrap-recovery \
    || die "the uninstall helper refused or failed; the lifecycle state was left intact"
}

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM

case "$(route)" in
  A) recover_uninstall ;;
  B)
    say "Resuming an interrupted Draft lifecycle operation at $install_root"
    "$installed_draft" __installer recover \
      || die "the installed Draft could not resume its interrupted operation; nothing else was changed"
    ;;
  E)
    die "$lifecycle holds uninstall residue with no journal and no terminal record; nothing was changed. This state needs lifecycle repair/support."
    ;;
esac
pending_recovery=0
[ "$(route)" = C ] && pending_recovery=1

# --- Download and stage-1 verification -----------------------------------

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

say "Downloading Draft $tag for $target"
download "$base_url/$asset" "$archive"
download "$base_url/SHA256SUMS" "$checksums"

expected="$(grep "  $asset\$" "$checksums" | awk '{print $1}')"
[ -n "$expected" ] || die "checksum entry for $asset was not found in SHA256SUMS"
[ "$(sha256_of "$archive")" = "$expected" ] || die "checksum verification failed for $asset"

mkdir "$tmp_dir/package"
tar -xzf "$archive" -C "$tmp_dir/package"
package_dir="$tmp_dir/package/draft-v${version}-${target}"
[ -x "$package_dir/bin/draft" ] || die "archive did not contain bin/draft"
[ -x "$package_dir/bin/draftd" ] || die "archive did not contain bin/draftd"
coordinator="$package_dir/bin/draft"

if [ "$pending_recovery" -eq 1 ]; then
  # Journal present, no bootstrap, no installed Draft: this downloaded
  # coordinator may continue only a compatible FreshInstall of exactly this
  # release. Anything else fails closed in Rust.
  say "Resuming an interrupted installation at $install_root"
  if ! "$coordinator" __installer recover --install-root "$install_root"; then
    [ -n "${DRAFT_INSTALL_ROOT:-}" ] && say "Keep DRAFT_INSTALL_ROOT=$install_root when re-running."
    die "the interrupted installation could not be resumed by this release; nothing was changed"
  fi
fi

# --- Install -------------------------------------------------------------

set -- __installer install --install-root "$install_root" --path-bin "$path_bin"
case "$migrate_legacy" in
  1|true) set -- "$@" --migrate-legacy ;;
esac
"$coordinator" "$@" || die "installation failed; see the message above"

case ":$PATH:" in
  *":$path_bin:"*) ;;
  *)
    say ""
    say "$path_bin is not on PATH."
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
      if ! grep -F "export PATH=\"$path_bin:\$PATH\"" "$profile" >/dev/null 2>&1; then
        {
          printf '\n# Draft CLI\n'
          printf 'export PATH="%s:$PATH"\n' "$path_bin"
        } >> "$profile"
      fi
      say "Updated PATH in $profile. Restart your shell or run:"
      say "  export PATH=\"$path_bin:\$PATH\""
    else
      say "Add it with:"
      say "  export PATH=\"$path_bin:\$PATH\""
      say "To let this installer update your shell profile, run with DRAFT_UPDATE_PATH=1."
    fi
    ;;
esac

"$install_root/bin/draft" --version || true
