#!/usr/bin/env bash
# Builds (and with --push, publishes) the Linux image Claktory's agent
# browser runs in on a Mac (#435), in podman, linux/arm64 only.
#
#   scripts/browser-image.sh base [--push]
#       The base: Ubuntu, Chrome stable, Xvfb, fonts, policy, firewall
#       (browser-image/Containerfile.base). Pushed as
#       ghcr.io/claktor/claktory-browser:base-<Chrome version>, and its digest
#       written to browser-image/base.txt (commit it). Run when Chrome moves.
#   scripts/browser-image.sh release VERSION RUNNER [--push]
#       One release's image: base.txt's base plus RUNNER (that release's
#       linux-aarch64 static factoryd) and the entrypoint. Pushed as
#       ghcr.io/claktor/claktory-browser:VERSION; prints the reference
#       factoryd is built with, CLAKTORY_BROWSER_IMAGE=REPO@sha256:…
#       BASE=REF overrides base.txt (a local base, to try one out).
#
# --push needs the vault's "GitHub packages token (browser image)" (GHCR_TOKEN).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO=ghcr.io/claktor/claktory-browser
ENGINE="${ENGINE:-podman}"
die() { echo "browser-image.sh: $*" >&2; exit 1; }
push=0; args=()
for a in "$@"; do [[ "$a" == --push ]] && push=1 || args+=("$a"); done
set -- "${args[@]}"
(($# >= 1)) || die "usage: browser-image.sh base|release VERSION RUNNER [--push]"

# Pushes $1 as $2; prints REPO@sha256:… . The token goes to a throwaway auth
# file, never the engine's own.
publish() {
  [[ -n "${GHCR_TOKEN:-}" ]] || die "--push needs GHCR_TOKEN (the vault's GitHub packages token)"
  local auth digest
  auth="$(mktemp -d)"
  trap 'rm -rf "$auth"' RETURN
  printf %s "$GHCR_TOKEN" | "$ENGINE" login --authfile "$auth/auth.json" -u "${GHCR_USER:-copiumnicus}" --password-stdin ghcr.io >/dev/null
  "$ENGINE" push --authfile "$auth/auth.json" --digestfile "$auth/digest" "$1" "docker://$2" >&2
  digest="$(cat "$auth/digest")"
  [[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || die "the push gave no digest"
  echo "$REPO@$digest"
}

case "$1" in
  base)
    "$ENGINE" build --platform linux/arm64 -f "$HERE/browser-image/Containerfile.base" -t localhost/claktory-browser:base "$HERE" >&2
    chrome="$("$ENGINE" run --rm --platform linux/arm64 --entrypoint google-chrome-stable localhost/claktory-browser:base --version | grep -oE '[0-9]+(\.[0-9]+){3}')"
    [[ -n "$chrome" ]] || die "Chrome didn't answer --version in the base image"
    "$ENGINE" tag localhost/claktory-browser:base "localhost/claktory-browser:base-$chrome"
    echo "base built: Chrome $chrome (localhost/claktory-browser:base-$chrome)" >&2
    if ((push)); then
      ref="$(publish "localhost/claktory-browser:base-$chrome" "$REPO:base-$chrome")"
      echo "$ref" >"$HERE/browser-image/base.txt"
      echo "pushed $REPO:base-$chrome; browser-image/base.txt is now $ref (commit it)" >&2
    fi ;;
  release)
    (($# == 3)) || die "usage: browser-image.sh release VERSION RUNNER [--push]"
    version="$2" runner="$3"
    [[ "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "$version isn't a release version (vX.Y.Z)"
    file -b "$runner" | grep -q 'ELF 64-bit.*ARM aarch64.*static' || die "$runner isn't a static linux-aarch64 binary"
    base="${BASE:-$(cat "$HERE/browser-image/base.txt" 2>/dev/null || true)}"
    [[ -n "$base" ]] || die "no base: run browser-image.sh base --push first (or set BASE)"
    ctx="$(mktemp -d)"
    trap 'rm -rf "$ctx"' EXIT
    cp "$runner" "$ctx/runner"
    cp "$HERE/browser-image/entry.sh" "$ctx/entry.sh"
    "$ENGINE" build --platform linux/arm64 -f "$HERE/browser-image/Containerfile" \
      --build-arg BASE="$base" --build-arg VERSION="$version" -t "localhost/claktory-browser:$version" "$ctx" >&2
    if ((push)); then
      ref="$(publish "localhost/claktory-browser:$version" "$REPO:$version")"
      echo "CLAKTORY_BROWSER_IMAGE=$ref"
    else
      echo "built localhost/claktory-browser:$version (not pushed)" >&2
    fi ;;
  *) die "doesn't know $1 (base or release)" ;;
esac
