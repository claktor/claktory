#!/usr/bin/env bash
# Installs Factory on a Mac, or its backend on a Linux machine, from its
# published releases, with no checkout, Rust or Flutter needed:
#
#   curl -fsSL <channel>/install.sh | bash
#
# scripts/publish.sh uploads this file as install.sh with each release, with
# the channel's manifest address filled in below. It downloads the newest
# release, checks it against the manifest, puts it in ~/.factory/releases/ as
# scripts/release.sh would, and installs it with the install-release.sh that
# comes in the release (on Linux, its install-linux.sh: a systemd user
# service, remote access on, and a pairing code for the Mac's app). Running it
# again repairs the install. Afterwards the backend checks the channel itself,
# and the app offers each new release.
#
#   FACTORY_UPDATE_URL=<manifest url>  another channel, such as a file:// one
#   install-online.sh --download-only DIR   only download and check the
#                                           release, into DIR/<version>
set -euo pipefail

# Filled in by scripts/publish.sh.
MANIFEST_URL=""

fail() {
  echo "Claktory can't be installed: $*" >&2
  exit 1
}

file_size() { if [[ "$(uname -s)" == Darwin ]]; then stat -f %z "$1"; else stat -c %s "$1"; fi; }
file_sha256() { { shasum -a 256 "$1" 2>/dev/null || sha256sum "$1"; } | cut -d' ' -f1; }

# A Linux machine has no plutil: the manifest is read as scripts/publish.sh
# writes it, a key on each line, and one line per Linux build:
#     "x86_64": {"url": "…", "sha256": "…", "size": 123},
top_field() { sed -n "s/^  \"$1\": \"\{0,1\}\([^\",]*\)\"\{0,1\},\{0,1\}\$/\1/p" "$2" | head -n 1; }
linux_field() { sed -n "s/.*\"$1\": \"\{0,1\}\([^\",}]*\).*/\1/p" <<<"$2"; }

# "15.7.3" -> 15007, to compare macOS versions.
macos_number() {
  local major minor
  IFS=. read -r major minor _ <<<"$1"
  echo $((10#${major:-0} * 1000 + 10#${minor:-0}))
}

main() {
  local url="${FACTORY_UPDATE_URL:-$MANIFEST_URL}" download_only=""
  if [[ "${1:-}" == "--download-only" ]]; then
    download_only="${2:?--download-only needs a folder}"
  fi
  [[ -n "$url" ]] || fail "this installer doesn't know where releases are published (set FACTORY_UPDATE_URL)."
  local os
  os="$(uname -s)"
  [[ "$os" == Darwin || "$os" == Linux ]] || fail "Claktory runs on macOS and Linux."

  # Global, for the trap that runs after main returns.
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT
  echo "Reading $url"
  curl -fsSL --proto '=https,file' "$url" -o "$work/latest.json" || fail "couldn't read $url"
  local version backend tarball sha256 size min_macos="" archs="" arch
  arch="$(uname -m)"
  if [[ "$os" == Linux ]]; then
    case "$arch" in
      x86_64 | amd64) arch=x86_64 ;;
      aarch64 | arm64) arch=aarch64 ;;
      *) fail "Claktory runs on x86_64 and aarch64 Linux, and this machine is $arch." ;;
    esac
    version="$(top_field version "$work/latest.json")"
    backend="$(top_field backend "$work/latest.json")"
    local build
    build="$(grep "^    \"$arch\": {" "$work/latest.json" || true)"
    [[ -n "$build" || -z "$version" ]] || fail "Claktory $version has no build for Linux $arch."
    tarball="$(linux_field url "$build")"
    sha256="$(linux_field sha256 "$build")"
    size="$(linux_field size "$build")"
  else
    field() { plutil -extract "$1" raw -o - "$work/latest.json" 2>/dev/null || true; }
    version="$(field version)"
    backend="$(field backend)"
    tarball="$(field url)"
    sha256="$(field sha256)"
    size="$(field size)"
    min_macos="$(field min_macos)"
    archs="$(plutil -extract archs json -o - "$work/latest.json" 2>/dev/null || true)"
  fi
  [[ "$version" =~ ^[A-Za-z0-9.-]+$ && -n "$tarball" && -n "$sha256" ]] || fail "the manifest at $url is incomplete."
  [[ "$backend" =~ ^[A-Za-z0-9.-]+$ ]] || backend="$version"

  if [[ -n "$min_macos" ]]; then
    local here
    here="$(sw_vers -productVersion)"
    (($(macos_number "$here") >= $(macos_number "$min_macos"))) || fail "it needs macOS $min_macos or later, and this Mac has $here."
  fi
  if [[ -n "$archs" && "$archs" != *"\"$arch\""* ]]; then
    fail "this release is built for $archs, and this Mac is $arch."
  fi

  echo "Downloading Claktory $version"
  curl -fSL --progress-bar --proto '=https,file' "$tarball" -o "$work/release.tar.gz" || fail "couldn't download $tarball"
  [[ -z "$size" || "$(file_size "$work/release.tar.gz")" == "$size" ]] || fail "the download's size doesn't match the manifest."
  [[ "$(file_sha256 "$work/release.tar.gz")" == "$(tr 'A-F' 'a-f' <<<"$sha256")" ]] ||
    fail "the download doesn't match the manifest's SHA-256."
  mkdir -p "$work/release"
  tar -xzf "$work/release.tar.gz" -C "$work/release"
  if [[ "$os" == Linux ]]; then
    [[ -f "$work/release/factoryd" && -f "$work/release/install-linux.sh" ]] || fail "the release is incomplete."
  else
    [[ -f "$work/release/factoryd" && -d "$work/release/Factory.app" && -f "$work/release/install-release.sh" ]] ||
      fail "the release is incomplete."
    codesign --verify --deep --strict "$work/release/Factory.app" || fail "the app's signature doesn't verify."
    if [[ -d "$work/release/supervisor/Claktory Agents.app" ]]; then
      codesign --verify --deep --strict "$work/release/supervisor/Claktory Agents.app" || fail "the supervisor's signature doesn't verify."
    fi
  fi
  echo "$backend" >"$work/release/backend-version"

  local releases="$HOME/.factory/releases"
  [[ -n "$download_only" ]] && releases="$download_only"
  mkdir -p "$releases"
  rm -rf "${releases:?}/$version"
  mv "$work/release" "$releases/$version"
  if [[ -n "$download_only" ]]; then
    echo "Claktory $version downloaded to $releases/$version"
    return
  fi
  # Named only once complete, as scripts/release.sh does.
  echo "$version" >"$releases/latest.new"
  mv -f "$releases/latest.new" "$releases/latest"

  if [[ "$os" == Linux ]]; then
    bash "$releases/$version/install-linux.sh" "$releases/$version" </dev/null
    echo
    echo "Claktory $version is installed and running here. Once your Mac's Claktory is connected to it, sign in to a model provider there, under Settings → Providers."
    return
  fi
  bash "$releases/$version/install-release.sh" "$releases/$version" --open </dev/null
  echo
  echo "Claktory $version is installed in ~/Applications and open."
  echo "Sign in to a model provider in Claktory, under Settings → Providers."
  if ! xcode-select -p >/dev/null 2>&1; then
    echo "Agents need git and other developer tools: run  xcode-select --install"
  fi
}

# Only once the whole script has arrived, when it's piped into bash.
main "$@"
