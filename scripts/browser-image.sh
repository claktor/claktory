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
#       saved as OUT_DIR/claktory-browser.oci.tar.zst, with the Safe Browsing
#       lists its Chrome fetched as it was built (seed.sh) also as
#       OUT_DIR/safe-browsing-seed.tar, for Linux releases. OUT_DIR/browser-image.env
#       gets what factoryd is built with:
#         CLAKTORY_BROWSER_REF=localhost/claktory-browser:VERSION  (what container image load prints)
#         CLAKTORY_BROWSER_IMAGE=claktory-browser@sha256:<the image's manifest digest>
#         CLAKTORY_BROWSER_ARCHIVE_SHA256=<the .zst's SHA-256>
#       The archive is checked on its own, without Apple's container: one
#       manifest, named VERSION's ref, whose blob hashes to the digest
#       factoryd pins, for linux/arm64, with its config and every layer
#       present at the hash and size the manifest gives. When container
#       answers too (an agent's commands can't reach it), the archive is also
#       loaded into it once and refused unless load prints that name and
#       inspect shows that one linux/arm64 variant; when it doesn't answer,
#       that extra check is skipped, never the release.
#
# CONTAINER=COMMAND names Apple's container command (container); a command
# that isn't there skips the extra check.
# BASE=REF uses that base instead of building one (to try one out).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENGINE="${ENGINE:-podman}"
CONTAINER="${CONTAINER:-container}"
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

# container_check REF ARCHIVE DIGEST: 0 when container loaded the archive
# as REF with the one variant linux/arm64@DIGEST; 1 when container didn't
# answer (skipped, with a note); dies when it answered with anything else.
container_check() {
  local ref="$1" archive="$2" digest="$3" started=false loaded seen
  if ! command -v "$CONTAINER" >/dev/null 2>&1; then
    echo "container isn't here: the extra check is skipped" >&2; return 1
  fi
  if ! "$CONTAINER" system status >/dev/null 2>&1; then
    if ! "$CONTAINER" system start --disable-kernel-install >/dev/null 2>&1; then
      echo "container doesn't answer: the extra check is skipped" >&2; return 1
    fi
    started=true
  fi
  "$CONTAINER" image delete "$ref" >/dev/null 2>&1 || true
  if ! loaded="$("$CONTAINER" image load -i "$archive" 2>/dev/null | tail -1)" || [[ -z "$loaded" ]]; then
    ! $started || "$CONTAINER" system stop >/dev/null 2>&1 || true
    echo "container didn't load the archive (not reachable from here): the extra check is skipped" >&2; return 1
  fi
  seen="$("$CONTAINER" image inspect "$ref" | python3 -c '
import json, sys
v = json.load(sys.stdin)[0]["variants"]
print(" ".join("%s/%s@%s" % (x["platform"]["os"], x["platform"]["architecture"], x["digest"]) for x in v))')" || seen="(inspect failed)"
  "$CONTAINER" image delete "$ref" >/dev/null 2>&1 || true
  ! $started || "$CONTAINER" system stop >/dev/null 2>&1 || true
  [[ "$loaded" == "$ref" ]] || die "container image load printed $loaded, not $ref"
  [[ "$seen" == "linux/arm64@$digest" ]] || die "container shows $seen, not linux/arm64@$digest"
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
    cp "$HERE/browser-image/entry.sh" "$HERE/browser-image/seed.sh" "$work/"
    # The seed is fetched under the policy the runner gives Chrome, so the
    # base's must be that one.
    "$ENGINE" run --rm --platform linux/arm64 --entrypoint cat "$base" /etc/opt/chrome/policies/managed/claktory.json >"$work/policy.json"
    python3 - "$HERE/../crates/factoryd/src/browser_runner/namespace.rs" "$work/policy.json" <<'PY' || die "the base's Chrome policy isn't the runner's POLICY (namespace.rs): rebuild the base"
import json, re, sys
src = open(sys.argv[1]).read()
m = re.search(r'pub\(crate\) const POLICY: &str = r#"(.*?)"#;', src)
sys.exit(0 if m and json.loads(m.group(1)) == json.load(open(sys.argv[2])) else 1)
PY
    "$ENGINE" build --platform linux/arm64 -f "$HERE/browser-image/Containerfile" \
      --build-arg BASE="$base" --build-arg VERSION="$version" --build-arg SEED_AT="$(date -u +%Y%m%dT%H%M%SZ)" -t "$ref" "$work" >&2
    # The seed again, for Linux releases, whose Chrome comes from apt:
    # browser-setup-linux.sh installs it at the same path.
    "$ENGINE" run --rm --platform linux/arm64 --entrypoint tar "$ref" -C /usr/local/share/claktory-browser -cf - safe-browsing >"$work/safe-browsing-seed.tar"
    python3 - "$work/safe-browsing-seed.tar" <<'PY' || die "the image's Safe Browsing seed isn't whole"
import re, sys, tarfile
names = {}
with tarfile.open(sys.argv[1]) as t:
    for m in t.getmembers():
        if m.isdir() and m.name.rstrip("/") == "safe-browsing":
            continue
        if not (m.isfile() and re.fullmatch(r"safe-browsing/[A-Za-z]+\.store(\.[0-9]+_[0-9]+)?", m.name)):
            sys.exit(f"unexpected {m.name}")
        names[m.name] = m.size
for l in ("UrlSoceng", "UrlMalware"):
    if not names.get(f"safe-browsing/{l}.store") or not any(re.fullmatch(rf"safe-browsing/{l}\.store\.[0-9]+_[0-9]+", n) and s > 0 for n, s in names.items()):
        sys.exit(f"no {l} list")
print(f"Safe Browsing seed: {len(names)} files, {sum(names.values()) // 1024} KB", file=sys.stderr)
PY
    # Layers uncompressed in the archive, so zstd compresses them all at once
    # (about 360 MB instead of 490 MB with podman's gzipped layers inside);
    # it unpacks to about 1.1 GB.
    "$ENGINE" save --uncompressed --format oci-archive -o "$work/image.oci.tar" "$ref"
    # The archive's one manifest: its digest is what factoryd pins and P6
    # checks after loading. Checked here from the archive itself, the same
    # fact container's inspect reports: the manifest blob hashes to it, and
    # it's linux/arm64 with every blob it names present and intact.
    mkdir "$work/x"
    tar -xf "$work/image.oci.tar" -C "$work/x"
    digest="$(python3 - "$work/x" "$ref" <<'PY'
import hashlib, json, os, sys
root, ref = sys.argv[1], sys.argv[2]
def fail(why):
    sys.exit(f"browser-image.sh: the archive {why}")
def blob(desc, media):
    if desc.get("mediaType") != media:
        fail(f"names a {desc.get('mediaType')} where a {media} belongs")
    algo, _, hexdigest = desc["digest"].partition(":")
    if algo != "sha256" or len(hexdigest) != 64:
        fail(f"has a digest that isn't sha256: {desc['digest']}")
    path = os.path.join(root, "blobs", "sha256", hexdigest)
    if not os.path.isfile(path) or os.path.islink(path):
        fail(f"is missing blob {desc['digest']}")
    h, size = hashlib.sha256(), 0
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk); size += len(chunk)
    if h.hexdigest() != hexdigest:
        fail(f"has blob {desc['digest']} that hashes to sha256:{h.hexdigest()}")
    if size != desc["size"]:
        fail(f"has blob {desc['digest']} of {size} bytes, not {desc['size']}")
    return path
index = json.load(open(os.path.join(root, "index.json")))
m = index["manifests"]
if len(m) != 1:
    fail("holds more than one manifest")
if m[0].get("annotations", {}).get("org.opencontainers.image.ref.name") != ref:
    fail("names another image")
if "platform" in m[0] and (m[0]["platform"].get("os"), m[0]["platform"].get("architecture")) != ("linux", "arm64"):
    fail(f"says its manifest is for {m[0]['platform']}")
manifest = json.load(open(blob(m[0], "application/vnd.oci.image.manifest.v1+json")))
config = json.load(open(blob(manifest["config"], "application/vnd.oci.image.config.v1+json")))
if (config.get("os"), config.get("architecture")) != ("linux", "arm64"):
    fail(f"is for {config.get('os')}/{config.get('architecture')}, not linux/arm64")
if not manifest["layers"]:
    fail("has no layers")
for layer in manifest["layers"]:
    blob(layer, layer.get("mediaType") if layer.get("mediaType") in ("application/vnd.oci.image.layer.v1.tar", "application/vnd.oci.image.layer.v1.tar+gzip", "application/vnd.oci.image.layer.v1.tar+zstd") else "an OCI layer")
if len(config.get("rootfs", {}).get("diff_ids", [])) != len(manifest["layers"]):
    fail("has a config whose layers don't match its manifest's")
print(m[0]["digest"])
PY
)"
    rm -rf "$work/x"
    [[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || die "the archive's manifest digest is $digest"
    mkdir -p "$out"
    zstd -q -19 -T0 -f "$work/image.oci.tar" -o "$out/claktory-browser.oci.tar.zst"
    cp "$work/safe-browsing-seed.tar" "$out/safe-browsing-seed.tar"
    sha="$(shasum -a 256 "$out/claktory-browser.oci.tar.zst" | cut -d' ' -f1)"
    echo "the archive checks out: $ref, linux/arm64 $digest" >&2
    # An extra check, when Apple's container answers: what it makes of the
    # archive, the same name and that one linux/arm64 variant, or the
    # release stops here. Anything that keeps container from answering (not
    # installed, not running and not startable, refused to an agent) skips
    # it with a note.
    if container_check "$ref" "$work/image.oci.tar" "$digest"; then
      echo "container loads it as $ref, linux/arm64 $digest" >&2
    fi
    printf 'CLAKTORY_BROWSER_REF=%s\nCLAKTORY_BROWSER_IMAGE=%s@%s\nCLAKTORY_BROWSER_ARCHIVE_SHA256=%s\n' "$ref" "$name" "$digest" "$sha" >"$out/browser-image.env"
    echo "archived $ref: $(($(stat -f %z "$out/claktory-browser.oci.tar.zst") / 1048576)) MB, $digest" >&2
    # Earlier versions of this image go once this one is archived: here they're
    # only build output, since a Claktory runs its browser from its release's
    # archive in Apple's container, never from this store. base and
    # base-<chrome> stay for the next build, and so does any version made in
    # the last 3 hours, which another build may still be saving.
    "$ENGINE" images --format json | python3 -c '
import json, re, sys, time
name, keep = sys.argv[1], sys.argv[2]
for image in json.load(sys.stdin):
    if image.get("Created", 0) > time.time() - 3 * 3600:
        continue
    for ref in image.get("Names") or []:
        repo, _, tag = ref.rpartition(":")
        if repo == f"localhost/{name}" and ref != keep and re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?", tag):
            print(ref)
' "$name" "$ref" | sort -u | while read -r old; do
      "$ENGINE" rmi "$old" >/dev/null 2>&1 && echo "removed the earlier $old" >&2 || true
    done || echo "earlier versions of $name weren't cleared (their list didn't read)" >&2
    ;;
  *) die "doesn't know $1 (base or archive)" ;;
esac
