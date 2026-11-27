#!/usr/bin/env bash
# Builds the Linux image Claktory's agent browser runs in on a Mac (#435), in
# podman, linux/arm64 only, and saves it as the release's OCI archive (W11:
# no registry; Cioccolata's P6 loads it into Apple's container and checks it).
#
#   scripts/browser-image.sh base
#       The base: Ubuntu, Chrome stable, Xvfb, fonts, firewall
#       (browser-image/Containerfile.base), built here as
#       localhost/claktory-browser:base-<Chrome version>. Prints that name.
#   scripts/browser-image.sh archive VERSION RUNNER OUT_DIR [--dev]
#       One release's image: a fresh base plus RUNNER (that release's
#       linux-aarch64 static factoryd) and the entrypoint, tagged
#       localhost/claktory-browser:VERSION (claktory-browser-dev with --dev),
#       saved as OUT_DIR/claktory-browser.oci.tar.zst. OUT_DIR/browser-image.env
#       gets what factoryd is built with:
#         CLAKTORY_BROWSER_REF=localhost/claktory-browser:VERSION  (what container image load prints)
#         CLAKTORY_BROWSER_IMAGE=claktory-browser@sha256:<the image's manifest digest>
#         CLAKTORY_BROWSER_ARCHIVE_SHA256=<the .zst's SHA-256>
#       When Apple's container is installed, the archive is loaded into it
#       once and refused unless load prints that name and inspect shows that
#       one linux/arm64 variant.
#
# BASE=REF uses that base instead of building one (to try one out).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENGINE="${ENGINE:-podman}"
die() { echo "browser-image.sh: $*" >&2; exit 1; }
(($# >= 1)) || die "usage: browser-image.sh base | archive VERSION RUNNER OUT_DIR [--dev]"

build_base() {
  "$ENGINE" build --platform linux/arm64 --pull=missing -f "$HERE/browser-image/Containerfile.base" -t localhost/claktory-browser:base "$HERE" >&2
  local chrome
  chrome="$("$ENGINE" run --rm --platform linux/arm64 --entrypoint google-chrome-stable localhost/claktory-browser:base --version | grep -oE '[0-9]+(\.[0-9]+){3}')"
  [[ -n "$chrome" ]] || die "Chrome didn't answer --version in the base image"
  "$ENGINE" tag localhost/claktory-browser:base "localhost/claktory-browser:base-$chrome"
  echo "base built: Chrome $chrome" >&2
  echo "localhost/claktory-browser:base-$chrome"
}

case "$1" in
  base) build_base ;;
  archive)
    (($# == 4 || $# == 5)) || die "usage: browser-image.sh archive VERSION RUNNER OUT_DIR [--dev]"
    version="$2" runner="$3" out="$4" name=claktory-browser
    [[ "${5:-}" != --dev ]] || name=claktory-browser-dev
    [[ "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || die "$version isn't a version (vX.Y.Z)"
    file -b "$runner" | grep -q 'ELF 64-bit.*ARM aarch64.*static' || die "$runner isn't a static linux-aarch64 binary"
    command -v zstd >/dev/null || die "zstd is needed (brew install zstd)"
    base="${BASE:-$(build_base)}"
    ref="localhost/$name:$version"
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT
    cp "$runner" "$work/runner"
    cp "$HERE/browser-image/entry.sh" "$work/entry.sh"
    "$ENGINE" build --platform linux/arm64 -f "$HERE/browser-image/Containerfile" \
      --build-arg BASE="$base" --build-arg VERSION="$version" -t "$ref" "$work" >&2
    # Layers uncompressed in the archive, so zstd compresses them all at once
    # (about 360 MB instead of 490 MB with podman's gzipped layers inside);
    # it unpacks to about 1.1 GB.
    "$ENGINE" save --uncompressed --format oci-archive -o "$work/image.oci.tar" "$ref"
    # The archive's one manifest: its digest is what P6 checks after loading.
    mkdir "$work/x"
    tar -xf "$work/image.oci.tar" -C "$work/x" index.json
    digest="$(python3 - "$work/x/index.json" "$ref" <<'PY'
import json, sys
index = json.load(open(sys.argv[1]))
m = index["manifests"]
assert len(m) == 1, "the archive holds more than one manifest"
assert m[0]["mediaType"] == "application/vnd.oci.image.manifest.v1+json", m[0]["mediaType"]
assert m[0].get("annotations", {}).get("org.opencontainers.image.ref.name") == sys.argv[2], "the archive names another image"
print(m[0]["digest"])
PY
)"
    [[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || die "the archive's manifest digest is $digest"
    mkdir -p "$out"
    zstd -q -19 -T0 -f "$work/image.oci.tar" -o "$out/claktory-browser.oci.tar.zst"
    sha="$(shasum -a 256 "$out/claktory-browser.oci.tar.zst" | cut -d' ' -f1)"
    # What container makes of it, when this Mac has container: the same name
    # and that one linux/arm64 variant, or the release stops here.
    if command -v container >/dev/null; then
      started=false
      if ! container system status >/dev/null 2>&1; then container system start --disable-kernel-install >/dev/null; started=true; fi
      container image delete "$ref" >/dev/null 2>&1 || true
      loaded="$(container image load -i "$work/image.oci.tar" | tail -1)"
      seen="$(container image inspect "$ref" | python3 -c '
import json, sys
v = json.load(sys.stdin)[0]["variants"]
print(" ".join("%s/%s@%s" % (x["platform"]["os"], x["platform"]["architecture"], x["digest"]) for x in v))')"
      container image delete "$ref" >/dev/null 2>&1 || true
      ! $started || container system stop >/dev/null 2>&1 || true
      [[ "$loaded" == "$ref" ]] || die "container image load printed $loaded, not $ref"
      [[ "$seen" == "linux/arm64@$digest" ]] || die "container shows $seen, not linux/arm64@$digest"
      echo "container loads it as $ref, linux/arm64 $digest" >&2
    fi
    printf 'CLAKTORY_BROWSER_REF=%s\nCLAKTORY_BROWSER_IMAGE=%s@%s\nCLAKTORY_BROWSER_ARCHIVE_SHA256=%s\n' "$ref" "$name" "$digest" "$sha" >"$out/browser-image.env"
    echo "archived $ref: $(($(stat -f %z "$out/claktory-browser.oci.tar.zst") / 1048576)) MB, $digest" >&2
    ;;
  *) die "doesn't know $1 (base or archive)" ;;
esac
