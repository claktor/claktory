#!/usr/bin/env bash
# Publishes this checkout as a release for Factories installed online (such as
# a friend's), not only for this Mac's.
#
# It runs scripts/release.sh with the channel's address stamped into the
# backend, so this Mac's Factory offers the update as always, and the
# backends it builds check the channel themselves. It builds the backend for
# Linux too (x86_64 and aarch64, in a container: scripts/build-linux.sh), for
# Factories whose backend runs on a Linux machine. Then it packs the release
# into tarballs, the Mac's and one per Linux architecture, and uploads them with:
#   latest.json   the manifest naming it, which installed backends read every
#                 half hour (crates/factoryd/src/channel.rs)
#   Claktory-<version>.dmg and Claktory.dmg  the Mac app to download and drag
#                 to Applications (scripts/make-dmg.sh); it sets Claktory up
#                 on first launch. latest.json names it as "dmg".
#   install.sh    the online installer (scripts/install-online.sh), for
#                 curl -fsSL https://github.com/claktory/claktory/releases/latest/download/install.sh | bash
#
#   scripts/publish.sh [--repo OWNER/NAME] [--notes TEXT] [--no-linux]
#       to GitHub Releases, claktory/claktory unless --repo names another.
#       Before building it checks that main is checked out and clean, and
#       that no secret is anywhere in its history (scripts/check-secrets.sh,
#       which needs gitleaks); after building, before uploading, it pushes
#       the release's source to the repository as one commit on top of the
#       previous release's (scripts/push-source.sh: never forced, except
#       once over a new repository's README-only first commit), so the
#       source of every release is public with it, and the day-to-day
#       commits aren't.
#   scripts/publish.sh --local DIR [--dev]
#       to a channel in a folder, to try it out; --dev releases for the dev
#       Factory. Point a backend at it with FACTORY_UPDATE_URL=file://DIR/latest.json.
#   scripts/publish.sh [--repo OWNER/NAME | --local DIR] --repoint VERSION
#       pulls the releases after VERSION: the channel serves VERSION's own
#       manifest again with a fresh issued_ms, signed again (on GitHub it
#       replaces that release's latest.json and marks it latest). Every
#       backend then withdraws the newer releases it downloaded. Never pull a
#       release by deleting it on GitHub: the older manifest that would come
#       back is refused as a replay, and the pulled one stays offered.
#   --no-linux  without the Linux builds (Linux backends then keep the release
#               they have)
#   --built     publishes the release already built (the newest in
#               ~/.factory/releases, or ~/.factory-dev's with --dev) instead
#               of building one: so a release can be built and signed first,
#               tried on this Mac, and then published exactly as tried. Build
#               it from main with the channel's address stamped in:
#                 FACTORY_UPDATE_URL=https://github.com/claktory/claktory/releases/latest/download/latest.json scripts/release.sh
#               It publishes exactly the release's own source, the commit
#               it was built from (its `source`), even once main has moved
#               on: the Linux builds, the install scripts and the source
#               pushed public all come from that commit. It refuses a
#               release built from a commit that isn't on main, or for
#               another channel.
#   --adhoc     allows a Mac app that isn't notarized (signed ad hoc): building
#               one (scripts/release.sh --adhoc) and publishing it to GitHub;
#               without it, both are refused.
#
# Agents: leave publishing to GitHub to Coordinator and the person.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/lib.sh"
# issued_ms (#478, Fugo's #181): when a manifest was issued, in unix
# milliseconds, inside what the signature covers. factoryd refuses a manifest
# older than the newest it accepted on the channel, so a replayed old one
# can't roll the offer back or withdraw a newer release. Prints one strictly
# above the one the channel serves now, whatever this Mac's clock says.
# $1: a scratch folder.
next_issued_ms() {
  local prev=0 code
  if [[ -n "$REPO" ]]; then
    code="$(curl -sSL --max-time 60 -o "$1/previous.json" -w '%{http_code}' "$MANIFEST_URL")" ||
      { echo "publish.sh: couldn't read the channel's current latest.json ($MANIFEST_URL), so issued_ms can't be checked against it" >&2; return 1; }
    case "$code" in
      200) ;;
      404) rm -f "$1/previous.json" ;;
      *) echo "publish.sh: the channel's latest.json answered HTTP $code; not publishing over a manifest it can't read" >&2; return 1 ;;
    esac
  elif [[ -f "$LOCAL/latest.json" ]]; then
    cp "$LOCAL/latest.json" "$1/previous.json"
  fi
  if [[ -f "$1/previous.json" ]]; then
    prev="$(python3 -c 'import json,sys; v=json.load(open(sys.argv[1])).get("issued_ms", 0); print(v if type(v) is int and v > 0 else 0)' "$1/previous.json")"
  fi
  python3 -c 'import sys, time; print(max(time.time_ns() // 1000000, int(sys.argv[1]) + 1))' "$prev"
}

# The publisher's signature over latest.json (G1, agreed with Security):
# ed25519 over the file's exact bytes, a one-line sidecar
# "claktory-release-1:BASE64" (FILE.sig) that factoryd verifies against its
# pinned public keys on https channels. The key lives only in the vault
# (CLAKTORY_SIGNING_KEY, a file): run publish.sh with that secret. Signed
# whenever the key is here; a publish to GitHub without it is refused.
# Old backends never fetch the sidecar, so it changes nothing for them.
SIGNING_KEY_ID="claktory-release-1"
sign_manifest() {
  local file="$1" dir
  dir="$(dirname "$file")"
  if [[ -n "${CLAKTORY_SIGNING_KEY:-}" ]]; then
    openssl version | grep -q "^OpenSSL 3" || { echo "signing needs OpenSSL 3 (brew install openssl@3); this openssl is: $(openssl version)" >&2; exit 1; }
    openssl pkeyutl -sign -inkey "$CLAKTORY_SIGNING_KEY" -rawin -in "$file" -out "$dir/latest.sig.bin"
    printf '%s:%s\n' "$SIGNING_KEY_ID" "$(base64 <"$dir/latest.sig.bin" | tr -d '\n')" >"$file.sig"
    # Self-check: the signature verifies against the key's own public half,
    # so a wrongly pasted vault value is caught here, never shipped. The
    # public key (not a secret) goes in the log.
    openssl pkey -in "$CLAKTORY_SIGNING_KEY" -pubout -out "$dir/latest.pub.pem"
    openssl pkeyutl -verify -pubin -inkey "$dir/latest.pub.pem" -rawin -in "$file" -sigfile "$dir/latest.sig.bin" >/dev/null ||
      { echo "the fresh signature doesn't verify: is CLAKTORY_SIGNING_KEY the release signing key?" >&2; exit 1; }
    echo "signed $(basename "$file") as $SIGNING_KEY_ID (public key $(openssl pkey -in "$dir/latest.pub.pem" -pubin -pubout -outform DER | tail -c 32 | base64))"
    rm -f "$dir/latest.sig.bin" "$dir/latest.pub.pem"
  elif [[ -n "$REPO" ]]; then
    echo "publish.sh: refusing to publish unsigned: factoryd verifies latest.json against the pinned release keys, so an unsigned publish would stop every update. Run publish.sh with the vault's CLAKTORY_SIGNING_KEY (the Claktory release signing key)." >&2
    exit 1
  fi
}

REPO="" LOCAL="" DEV=() NOTES="" LINUX=(x86_64 aarch64) BUILT=false ADHOC=false REPOINT=""
DEFAULT_REPO="claktory/claktory"
usage() {
  echo "usage: scripts/publish.sh [--repo OWNER/NAME] [--notes TEXT] [--no-linux] [--built] [--adhoc] | --local DIR [--dev] [--no-linux] [--built]" >&2
  echo "       scripts/publish.sh [--repo OWNER/NAME | --local DIR] --repoint VERSION" >&2
  exit 2
}
while (($#)); do
  case "$1" in
    --repo) REPO="${2:-}"; shift 2 || usage ;;
    --local) LOCAL="${2:-}"; shift 2 || usage ;;
    --notes) NOTES="${2:-}"; shift 2 || usage ;;
    --dev) DEV=(--dev); shift ;;
    --no-linux) LINUX=(); shift ;;
    --built) BUILT=true; shift ;;
    --adhoc) ADHOC=true; shift ;;
    --repoint) REPOINT="${2:-}"; shift 2 || usage ;;
    *) usage ;;
  esac
done
if [[ -n "$REPO" && -n "$LOCAL" ]]; then usage; fi
[[ -n "$LOCAL" ]] || REPO="${REPO:-$DEFAULT_REPO}"

# --repoint VERSION: pulls the releases after VERSION (#478, Bruno 34111).
# The channel serves VERSION's own manifest again, byte for byte as it was
# published but for a fresh issued_ms, signed again, so every backend accepts
# it as the newest manifest, withdraws the releases after VERSION it
# downloaded (#456) and never offers them again. Letting GitHub's
# releases/latest fall back to an older release's manifest no longer works:
# that one's issued_ms is older, and backends refuse it as a replay.
if [[ -n "$REPOINT" ]]; then
  [[ "$REPOINT" =~ ^v[0-9A-Za-z.-]+$ ]] || usage
  [[ ${#DEV[@]} -eq 0 && -z "$NOTES" && ${#LINUX[@]} -eq 2 ]] && ! $BUILT && ! $ADHOC || usage
  STAGE="$(mktemp -d)"
  trap 'rm -rf "$STAGE"' EXIT
  mkdir "$STAGE/old"
  if [[ -n "$REPO" ]]; then
    [[ "$REPO" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || usage
    [[ -n "${CLAKTORY_SIGNING_KEY:-}" ]] || { echo "publish.sh: --repoint signs the manifest: run it with the vault's CLAKTORY_SIGNING_KEY" >&2; exit 1; }
    BASE="https://github.com/$REPO/releases"
    MANIFEST_URL="$BASE/latest/download/latest.json"
    gh release download "$REPOINT" --repo "$REPO" --pattern latest.json --pattern latest.json.sig --dir "$STAGE/old"
  else
    LOCAL="$(cd "$LOCAL" && pwd)"
    MANIFEST_URL="file://$LOCAL/latest.json"
    [[ -f "$LOCAL/$REPOINT/latest.json" ]] || { echo "publish.sh: $LOCAL has no $REPOINT/latest.json" >&2; exit 1; }
    cp "$LOCAL/$REPOINT/latest.json" "$STAGE/old/"
    [[ ! -f "$LOCAL/$REPOINT/latest.json.sig" ]] || cp "$LOCAL/$REPOINT/latest.json.sig" "$STAGE/old/"
  fi
  # Only a manifest we signed is signed again: its own signature must verify
  # with the release key (always for GitHub).
  if [[ -n "${CLAKTORY_SIGNING_KEY:-}" && ( -n "$REPO" || -f "$STAGE/old/latest.json.sig" ) ]]; then
    [[ -f "$STAGE/old/latest.json.sig" ]] || { echo "publish.sh: $REPOINT's manifest has no signature; not signing it again" >&2; exit 1; }
    [[ "$(cut -d: -f1 "$STAGE/old/latest.json.sig")" == "$SIGNING_KEY_ID" ]] || { echo "publish.sh: $REPOINT's manifest is signed by another key ($(cut -d: -f1 "$STAGE/old/latest.json.sig")); not signing it again" >&2; exit 1; }
    cut -d: -f2 "$STAGE/old/latest.json.sig" | tr -d '\n' | base64 -d >"$STAGE/old.sig.bin"
    openssl pkey -in "$CLAKTORY_SIGNING_KEY" -pubout -out "$STAGE/old.pub.pem"
    openssl pkeyutl -verify -pubin -inkey "$STAGE/old.pub.pem" -rawin -in "$STAGE/old/latest.json" -sigfile "$STAGE/old.sig.bin" >/dev/null ||
      { echo "publish.sh: $REPOINT's published manifest doesn't verify with the release key; not signing it again" >&2; exit 1; }
  fi
  ISSUED_MS="$(next_issued_ms "$STAGE")"
  python3 - "$STAGE/old/latest.json" "$STAGE/latest.json" "$REPOINT" "$ISSUED_MS" <<'PYREPOINT'
import json, re, sys
src, out, version, issued = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
text = open(src).read()
if json.loads(text).get("version") != version:
    sys.exit(f"publish.sh: that manifest names {json.loads(text).get('version')}, not {version}")
line = f'  "issued_ms": {issued},\n'
if re.search(r'^  "issued_ms": \d+,\n', text, re.M):
    text = re.sub(r'^  "issued_ms": \d+,\n', line, text, count=1, flags=re.M)
else:
    m = re.search(r'^  "released_at": "[^"\n]*",\n', text, re.M) or re.search(r'^  "version": "[^"\n]*",\n', text, re.M)
    if not m:
        sys.exit("publish.sh: that manifest has no released_at or version line to stamp after")
    text = text[:m.end()] + line + text[m.end():]
new = json.loads(text)
old = json.loads(open(src).read())
new.pop("issued_ms"); old.pop("issued_ms", None)
assert new == old, "only issued_ms may change"
open(out, "w").write(text)
PYREPOINT
  sign_manifest "$STAGE/latest.json"
  if [[ -n "$REPO" ]]; then
    gh release upload "$REPOINT" "$STAGE/latest.json" "$STAGE/latest.json.sig" --repo "$REPO" --clobber
    gh release edit "$REPOINT" --repo "$REPO" --latest >/dev/null
    for _ in $(seq 1 30); do
      cmp -s <(curl -fsSL --max-time 30 "$MANIFEST_URL" 2>/dev/null) "$STAGE/latest.json" && break
      sleep 10
    done
    cmp -s <(curl -fsSL --max-time 30 "$MANIFEST_URL") "$STAGE/latest.json" ||
      { echo "publish.sh: $MANIFEST_URL doesn't serve the new manifest yet; check the release page" >&2; exit 1; }
  else
    for f in latest.json latest.json.sig; do
      [[ -f "$STAGE/$f" ]] || continue
      cp "$STAGE/$f" "$LOCAL/$REPOINT/$f"
      cp "$STAGE/$f" "$LOCAL/$f.new"; mv -f "$LOCAL/$f.new" "$LOCAL/$f"
    done
  fi
  echo "the channel serves $REPOINT again (issued_ms $ISSUED_MS): backends withdraw the releases after it that they downloaded"
  exit 0
fi

if [[ -n "$REPO" ]]; then
  [[ "$REPO" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || usage
  [[ ${#DEV[@]} -eq 0 ]] || { echo "--dev releases only go to a --local channel" >&2; exit 2; }
  # Before building: is the repository there, and can gh reach it?
  gh repo view "$REPO" --json name >/dev/null
  # Before building: the source to push is main, clean, without secrets. The
  # dry run checks all of it (and what the repository holds) and pushes nothing.
  # A release built before (--built) is checked below, with its own source.
  if ! $BUILT; then
    SOURCE="$(git -C "$ROOT" rev-parse HEAD)"
    "$ROOT/scripts/push-source.sh" "https://github.com/$REPO.git" --source "$SOURCE" --dry-run
  fi
  BASE="https://github.com/$REPO/releases"
  MANIFEST_URL="$BASE/latest/download/latest.json"
else
  mkdir -p "$LOCAL"
  LOCAL="$(cd "$LOCAL" && pwd)"
  MANIFEST_URL="file://$LOCAL/latest.json"
fi

if [[ ${#LINUX[@]} -gt 0 ]]; then
  # Before building: the Linux builds need a container engine that runs.
  ENGINE="${FACTORY_CONTAINER:-$(command -v podman || command -v docker || true)}"
  [[ -n "$ENGINE" ]] && "$ENGINE" info >/dev/null 2>&1 || {
    echo "the Linux builds need podman or docker, running (or publish with --no-linux)" >&2
    exit 1
  }
  # x86_64 builds never run on an arm64 Mac: they need the x86_64 host, and
  # its podman, before anything is built (scripts/lib.sh container_for).
  if [[ " ${LINUX[*]} " == *" x86_64 "* ]]; then
    container_for x86_64 || exit 1
    if [[ -n "$X86_REMOTE" ]] && ! x86_ssh 'podman info >/dev/null 2>&1' </dev/null; then
      echo "the x86_64 Linux build runs on $X86_REMOTE, which doesn't answer with a working podman over ssh" >&2
      exit 1
    fi
  fi
fi

if [[ ${#DEV[@]} -eq 0 ]]; then
  HOME_DIR="$HOME/.factory" APP_NAME="Factory.app"
else
  HOME_DIR="$HOME/.factory-dev" APP_NAME="Factory Dev.app"
fi

# A Mac app on GitHub is notarized, unless --adhoc says otherwise.
require_notarized() {
  [[ -n "$REPO" ]] && ! $ADHOC || return 0
  spctl --assess --type execute "$1" 2>/dev/null && xcrun stapler validate -q "$1" 2>/dev/null || {
    echo "$1 isn't notarized (scripts/sign-release.sh signs releases built with the App Store Connect key); publish it anyway with --adhoc" >&2
    exit 1
  }
}

# Browser control's helper goes public only once it has passed its release
# proof (B6b): until then a release that carries it is a rehearsal's.
require_no_browser_host() {
  # A --dev release (Claktory Dev, only ever to a --local channel) always
  # carries the helper's dev build (B4b); only real releases wait for B6b.
  [[ ${#DEV[@]} -eq 0 ]] || return 0
  [[ ! -e "$1/Contents/Helpers/claktory-browser-host" ]] || {
    echo "$1 carries the browser helper, which isn't published before browser control passes B6b" >&2
    exit 1
  }
}

# A release page opens with a one-line summary for people (Bruno 40617, Trish
# 40653): the plain line under the version's "Released … UTC." line in the
# source's docs/CHANGELOG.md (scripts/changelog.sh says how it's written).
# Checked before anything is built or pushed; GitHub publishes only.
release_summary() { # VERSION (empty: the newest) COMMIT
  local file summary
  file="$(mktemp)"
  git -C "$ROOT" show "$2:docs/CHANGELOG.md" >"$file"
  summary="$("$ROOT/scripts/changelog.sh" summary "$1" "$file")" || { rm -f "$file"; return 1; }
  rm -f "$file"
  echo "$summary"
}

if $BUILT; then
  VERSION="$(cat "$HOME_DIR/releases/latest" 2>/dev/null)" || { echo "no release in $HOME_DIR/releases to publish" >&2; exit 1; }
  RELEASE="$HOME_DIR/releases/$VERSION"
  # The release is exactly its own source: the commit it was built from,
  # which must be on main (this checkout's HEAD, which push-source.sh
  # requires to be main).
  SOURCE="$(cat "$RELEASE/source" 2>/dev/null || true)"
  [[ -n "$SOURCE" ]] && git -C "$ROOT" merge-base --is-ancestor "$SOURCE" HEAD 2>/dev/null || {
    echo "$VERSION was built from ${SOURCE:-an unrecorded commit}, which isn't in this checkout's history; publish it from main" >&2
    exit 1
  }
  [[ "$SOURCE" == "$(git -C "$ROOT" rev-parse HEAD)" ]] || echo "$VERSION was built from ${SOURCE:0:10}; publishing that commit's source, Linux builds and install scripts"
  if [[ -n "$REPO" && -z "$NOTES" ]]; then release_summary "$VERSION" "$SOURCE" >/dev/null || exit 1; fi
  if [[ -n "$REPO" ]]; then
    "$ROOT/scripts/push-source.sh" "https://github.com/$REPO.git" --source "$SOURCE" --dry-run
  fi
  [[ "$(cat "$RELEASE/update-url" 2>/dev/null)" == "$MANIFEST_URL" ]] || {
    echo "$VERSION's backend checks \"$(cat "$RELEASE/update-url" 2>/dev/null || true)\" for updates, not this channel; build it with:" >&2
    echo "  FACTORY_UPDATE_URL=$MANIFEST_URL scripts/release.sh ${DEV[*]-}" >&2
    exit 1
  }
  if [[ -n "$REPO" ]] && gh release view "$VERSION" --repo "$REPO" >/dev/null 2>&1; then
    echo "$VERSION is already published to $REPO" >&2
    exit 1
  fi
  require_notarized "$RELEASE/$APP_NAME"
  require_no_browser_host "$RELEASE/$APP_NAME"
  echo "publishing $VERSION as built ($RELEASE)"
else
  # Before building: does the version it builds (the newest in this
  # checkout's changelog) have its summary?
  if [[ -n "$REPO" && -z "$NOTES" && ${#DEV[@]} -eq 0 ]]; then release_summary "" HEAD >/dev/null || exit 1; fi
  # Before building: would it be notarized?
  if [[ -n "$REPO" ]] && ! $ADHOC; then
    [[ "$(signing_mode)" != adhoc ]] || {
      echo "this release would be signed ad hoc, not notarized: it needs the Developer ID identity and the App Store Connect key (see scripts/sign-release.sh); publish it anyway with --adhoc" >&2
      exit 1
    }
  fi
  RELEASE_ARGS=(${DEV[@]+"${DEV[@]}"})
  if $ADHOC && [[ ${#DEV[@]} -eq 0 ]]; then RELEASE_ARGS+=(--adhoc); fi
  FACTORY_UPDATE_URL="$MANIFEST_URL" "$ROOT/scripts/release.sh" ${RELEASE_ARGS[@]+"${RELEASE_ARGS[@]}"}
  VERSION="$(cat "$HOME_DIR/releases/latest")"
  RELEASE="$HOME_DIR/releases/$VERSION"
  SOURCE="$(cat "$RELEASE/source")"
  require_notarized "$RELEASE/$APP_NAME"
  require_no_browser_host "$RELEASE/$APP_NAME"
fi
NAME="factory-$VERSION-macos.tar.xz"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
# What ships beside the app (the license, the install scripts, the Linux
# builds) comes from the release's source commit, exported into the private
# build folder (scripts/lib.sh release_tree); a dev release's from this
# checkout, as it was built.
if [[ ${#DEV[@]} -eq 0 ]]; then
  SRC="$(release_tree "$ROOT" "${SOURCE:-HEAD}")"
  # The Linux builds keep nothing from earlier ones: volumes of this run's
  # own, removed after (scripts/build-linux.sh, FACTORY_LINUX_FRESH).
  export FACTORY_LINUX_FRESH=1 FACTORY_LINUX_RUN="publish-$$-$(date +%s)"
  linux_run_cleanup() {
    local engine="${FACTORY_CONTAINER:-podman}" arch
    command -v "$engine" >/dev/null || engine=docker
    for arch in x86_64 aarch64; do "$engine" volume rm -f "factory-linux-run-$FACTORY_LINUX_RUN-target-$arch" >/dev/null 2>&1 || true; done
    "$engine" volume rm -f "factory-linux-run-$FACTORY_LINUX_RUN-cargo" >/dev/null 2>&1 || true
    # And the x86_64 host's, where its builds ran.
    if [[ -n "${X86_REMOTE:-}" ]]; then
      x86_ssh "podman volume rm -f factory-linux-run-$FACTORY_LINUX_RUN-target-x86_64 factory-linux-run-$FACTORY_LINUX_RUN-cargo" </dev/null >/dev/null 2>&1 || true
    fi
  }
  trap 'rm -rf "$STAGE"; linux_run_cleanup; release_unlock' EXIT
  # The real path: the container engine's VM shares /private, not /tmp.
  SRC="$(cd "$SRC" && pwd -P)"
else
  SRC="$ROOT"
fi
# The license travels with every copy (PolyForm Shield).
# The supervisor's app, in releases that have one (scripts/lib.sh make_supervisor_app).
SUPERVISOR_DIR=()
[[ -d "$RELEASE/supervisor" ]] && SUPERVISOR_DIR=(supervisor)
# Claktory's own apps (apps/), in releases that ship them.
APPS_DIR=()
[[ -d "$RELEASE/apps" ]] && APPS_DIR=(apps)
# The video preview helper (#379), in Mac releases that have it.
VIDEO_PREVIEW=()
[[ -f "$RELEASE/claktory-video-preview" ]] && VIDEO_PREVIEW=(claktory-video-preview)
# Agents' tools (tools/rg, tools/jq and their licenses), in releases that have them.
TOOLS_DIR=()
[[ -d "$RELEASE/tools" ]] && TOOLS_DIR=(tools)
# Smaller Mac updates (#454, efficiency/release-downloads.md): the signed
# release goes into a staging copy (clones, free on APFS) where files with
# the same bytes and mode become hard links to one another (factoryd and the
# other helpers sit both at the top and in the app, and the supervisor's app
# is factoryd again), so tar stores each once; then xz with a dictionary
# large enough to see the whole archive. The release itself and its
# signatures stay as they are. Every Mac backend and the online installer
# unpack it with the same `tar -xzf` as before: macOS's tar reads xz by its
# content, and restores the hard links. Linux stays gzip (GNU tar's -z).
command -v xz >/dev/null || { echo "publish.sh: xz is needed for the Mac tarball (brew install xz)" >&2; exit 1; }
PACK="$STAGE/pack"
mkdir "$PACK"
MAC_ITEMS=("$APP_NAME" factoryd claktory-apps claktory-speech ${VIDEO_PREVIEW[@]+"${VIDEO_PREVIEW[@]}"} backend-version CHANGELOG.md ${SUPERVISOR_DIR[@]+"${SUPERVISOR_DIR[@]}"} ${APPS_DIR[@]+"${APPS_DIR[@]}"} ${TOOLS_DIR[@]+"${TOOLS_DIR[@]}"})
for item in "${MAC_ITEMS[@]}"; do cp -cRp "$RELEASE/$item" "$PACK/"; done
python3 - "$PACK" <<'PYLINK'
import hashlib, os, stat, sys
groups = {}
for top, dirs, files in os.walk(sys.argv[1]):
    dirs.sort()
    for f in sorted(files):
        p = os.path.join(top, f)
        st = os.lstat(p)
        if stat.S_ISREG(st.st_mode) and st.st_size >= 65536:
            h = hashlib.sha256()
            with open(p, "rb") as fh:
                for chunk in iter(lambda: fh.read(1 << 20), b""):
                    h.update(chunk)
            groups.setdefault((st.st_size, stat.S_IMODE(st.st_mode), h.hexdigest()), []).append(p)
saved = 0
for (size, _, _), paths in groups.items():
    for p in paths[1:]:
        os.unlink(p)
        os.link(paths[0], p)
        saved += size
print(f"hard-linked identical files: {saved // (1 << 20)} MB stored once instead of twice")
PYLINK
COPYFILE_DISABLE=1 tar --no-xattrs --uid 0 --gid 0 --uname "" --gname "" -cf - \
  -C "$PACK" "${MAC_ITEMS[@]}" \
  -C "$SRC" LICENSE.md \
  -C "$SRC/scripts" install-release.sh | xz -T1 --lzma2=preset=9e,dict=192MiB >"$STAGE/$NAME"
rm -rf "$PACK"
echo "Mac tarball: $NAME, $(($(stat -f %z "$STAGE/$NAME") / 1048576)) MB"

# Never QA's provider override (Fugo, #332), whatever built this release.
check_no_qa_provider "$RELEASE/factoryd" || exit 1
# The real app carries the backend as a helper too; a dev release's doesn't.
if [[ "$APP_NAME" == "Factory.app" ]]; then
  check_no_qa_provider "$RELEASE/$APP_NAME/Contents/Helpers/factoryd" || exit 1
fi
ARCHS="$(lipo -archs "$RELEASE/factoryd" | sed 's/[^ ][^ ]*/"&"/g; s/ /, /g')"
MIN_MACOS="$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$RELEASE/$APP_NAME/Contents/Info.plist" 2>/dev/null || echo 12.0)"
# Where an uploaded file ends up.
url_of() {
  if [[ -n "$REPO" ]]; then echo "$BASE/download/$VERSION/$1"; else echo "file://$LOCAL/$VERSION/$1"; fi
}
URL="$(url_of "$NAME")"

# Linux: the backend alone, static, stamped with the backend's version (an
# older one when this release only changed the app) and the channel.
BACKEND_VERSION="$(cat "$RELEASE/backend-version")"
FILES=("$STAGE/$NAME")
LINUX_JSON=""
for arch in ${LINUX[@]+"${LINUX[@]}"}; do
  build="$STAGE/linux-$arch"
  mkdir -p "$build"
  FACTORY_LINUX_SRC="$SRC" FACTORY_VERSION="$BACKEND_VERSION" FACTORY_UPDATE_URL="$MANIFEST_URL" "$ROOT/scripts/build-linux.sh" "$arch" "$build/factoryd"
  FACTORY_LINUX_SRC="$SRC" "$ROOT/scripts/build-linux.sh" "$arch" "$build/claktory-apps" claktory-apps
  FACTORY_LINUX_SRC="$SRC" "$ROOT/scripts/build-linux.sh" "$arch" "$build/claktory-speech" claktory-speech
  check_no_qa_provider "$build/factoryd" || exit 1
  cp "$RELEASE/backend-version" "$RELEASE/CHANGELOG.md" "$build/"
  [[ ! -d "$RELEASE/apps" ]] || cp -R "$RELEASE/apps" "$build/apps"
  # ripgrep and jq, from the release's own pins (its source's build-rg.sh
  # and build-jq.sh), each when the Mac release has it.
  if [[ ${#TOOLS_DIR[@]} -gt 0 ]]; then
    [[ ! -f "$RELEASE/tools/rg" ]] || "$SRC/scripts/build-rg.sh" linux "$arch" "$build/tools/rg"
    [[ ! -f "$RELEASE/tools/jq" ]] || "$SRC/scripts/build-jq.sh" linux "$arch" "$build/tools/jq"
    cp -R "$RELEASE/tools/LICENSES" "$build/tools/LICENSES"
  fi
  # claktory-wsl beside factoryd too (#490), under the name updates put in
  # ~/.factory/bin, so an install updated from before it has it there.
  cp "$SRC/scripts/claktory-wsl.sh" "$build/claktory-wsl"
  chmod 755 "$build/claktory-wsl"
  linux_name="factory-$VERSION-linux-$arch.tar.gz"
  COPYFILE_DISABLE=1 tar --no-xattrs --uid 0 --gid 0 --uname "" --gname "" -czf "$STAGE/$linux_name" \
    -C "$build" factoryd claktory-apps claktory-speech claktory-wsl backend-version CHANGELOG.md ${APPS_DIR[@]+"${APPS_DIR[@]}"} ${TOOLS_DIR[@]+"${TOOLS_DIR[@]}"} \
    -C "$SRC" LICENSE.md \
    -C "$SRC/scripts" install-linux.sh claktory-wsl.sh
  FILES+=("$STAGE/$linux_name")
  # One line per build: the online installer reads it with sed, on machines without jq.
  [[ -n "$LINUX_JSON" ]] && LINUX_JSON+=$',\n'
  LINUX_JSON+="    \"$arch\": {\"url\": \"$(url_of "$linux_name")\", \"sha256\": \"$(shasum -a 256 "$STAGE/$linux_name" | cut -d' ' -f1)\", \"size\": $(stat -f %z "$STAGE/$linux_name")}"
done

# The disk image people download to install on a Mac (scripts/make-dmg.sh):
# Claktory-<version>.dmg, and the same as Claktory.dmg, so
# releases/latest/download/Claktory.dmg (claktory.com/download/mac) is always
# the newest. Signed and notarized for GitHub; unsigned for a --local channel
# or with --adhoc.
DMG_JSON=""
if [[ ${#DEV[@]} -eq 0 ]]; then
  DMG_ARGS=()
  if [[ -z "$REPO" ]] || $ADHOC; then DMG_ARGS=(--unsigned); fi
  DMG="$("$ROOT/scripts/make-dmg.sh" "$RELEASE" "$STAGE" ${DMG_ARGS[@]+"${DMG_ARGS[@]}"})"
  cp "$DMG" "$STAGE/Claktory.dmg"
  FILES+=("$DMG" "$STAGE/Claktory.dmg")
  DMG_JSON="  \"dmg\": {\"url\": \"$(url_of "$(basename "$DMG")")\", \"sha256\": \"$(shasum -a 256 "$DMG" | cut -d' ' -f1)\", \"size\": $(stat -f %z "$DMG")},"
fi

# The agent browser's Mac image (#435 W11), with the release that built it,
# under the name its factoryd downloads (CLAKTORY_BROWSER_ARCHIVE_URL,
# compiled in, as is its SHA-256). A release that reuses an older backend
# carries that backend's image, already uploaded with its own release.
if [[ -f "$RELEASE/claktory-browser.oci.tar.zst" && "$BACKEND_VERSION" == "$VERSION" ]]; then
  browser_name="claktory-browser-$VERSION-arm64.oci.tar.zst"
  want_url="$(sed -n 's/^CLAKTORY_BROWSER_ARCHIVE_URL=//p' "$RELEASE/browser-image.env")"
  want_sha="$(sed -n 's/^CLAKTORY_BROWSER_ARCHIVE_SHA256=//p' "$RELEASE/browser-image.env")"
  if [[ -n "$REPO" && "$want_url" != "$(url_of "$browser_name")" ]]; then
    echo "the browser image's address compiled into factoryd, ${want_url:-none}, isn't where this publishes it, $(url_of "$browser_name")" >&2
    exit 1
  fi
  [[ "$(shasum -a 256 "$RELEASE/claktory-browser.oci.tar.zst" | cut -d' ' -f1)" == "$want_sha" ]] ||
    { echo "$RELEASE/claktory-browser.oci.tar.zst isn't the image factoryd was built for" >&2; exit 1; }
  cp -c "$RELEASE/claktory-browser.oci.tar.zst" "$STAGE/$browser_name"
  FILES+=("$STAGE/$browser_name")
fi

# The last gate before anything is signed or uploaded: no binary going
# public, the Mac's or Linux's, carries this Mac's home folder, user or
# host name (check_no_traces in lib.sh; release.sh checks the same before
# signing).
# A --dev release (only ever to a --local channel, never public) is built
# without the remapping, so it's left out.
[[ ${#DEV[@]} -gt 0 ]] || check_no_traces "$RELEASE" "$STAGE"
# The same for the archives' headers (owner, group, names), and those of
# every .clapp inside them: gitleaks and the byte search above don't see
# them compressed (check_tar_headers in lib.sh).
check_tar_headers "$STAGE"/*.tar.?z

# When this manifest was issued (#478): next_issued_ms, above.
ISSUED_MS="$(next_issued_ms "$STAGE")"

# The top-level url is the Mac's, as every backend so far reads it; Linux
# backends read theirs under "linux".
cat >"$STAGE/latest.json" <<JSON
{
  "version": "$VERSION",
  "released_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "issued_ms": $ISSUED_MS,
  "backend": "$(cat "$RELEASE/backend-version")",
  "url": "$URL",
  "sha256": "$(shasum -a 256 "$STAGE/$NAME" | cut -d' ' -f1)",
  "size": $(stat -f %z "$STAGE/$NAME"),
  "min_macos": "$MIN_MACOS",
  "archs": [$ARCHS],
${DMG_JSON:+$DMG_JSON
}  "linux": {
$LINUX_JSON
  }
}
JSON
sign_manifest "$STAGE/latest.json"
[[ ! -f "$STAGE/latest.json.sig" ]] || FILES+=("$STAGE/latest.json.sig")

sed "s|^MANIFEST_URL=\"\"\$|MANIFEST_URL=\"$MANIFEST_URL\"|" "$SRC/scripts/install-online.sh" >"$STAGE/install.sh"
grep -q "^MANIFEST_URL=\"$MANIFEST_URL\"\$" "$STAGE/install.sh" || { echo "couldn't fill in install.sh" >&2; exit 1; }

if [[ -n "$REPO" ]]; then
  # The source first: a release is never public without its source.
  "$ROOT/scripts/push-source.sh" "https://github.com/$REPO.git" --version "$VERSION" --source "$SOURCE"
  # Its tag is the release's public commit, which push-source.sh left as the branch public.
  PUBLIC_COMMIT="$(git -C "$ROOT" rev-parse refs/heads/public)"
  # The release page says what the version brings: the changelog section,
  # which is the public commit's own message body (public_commit in lib.sh).
  # Its summary first, then the rest of the section without it.
  if [[ -z "$NOTES" ]]; then
    NOTES="$(git -C "$ROOT" log -1 --format=%b refs/heads/public)"
    [[ -n "$NOTES" ]] || NOTES="Claktory $VERSION"
    if [[ ${#DEV[@]} -eq 0 ]]; then
      SUMMARY="$(release_summary "$VERSION" "$SOURCE")" || exit 1
      NOTES="$SUMMARY"$'\n\n'"$(grep -vxF -- "$SUMMARY" <<<"$NOTES" || true)"
    fi
  fi
  gh release create "$VERSION" "${FILES[@]}" "$STAGE/latest.json" "$STAGE/install.sh" \
    --repo "$REPO" --target "$PUBLIC_COMMIT" --title "Claktory $VERSION" --notes "$NOTES"
  echo "published Claktory $VERSION to $BASE"
  echo "install on another Mac, or a Linux machine, with:"
  echo "  curl -fsSL $BASE/latest/download/install.sh | bash"
else
  mkdir -p "$LOCAL/$VERSION"
  cp "${FILES[@]}" "$LOCAL/$VERSION/"
  # Each version's manifest beside its files, as GitHub keeps it (--repoint).
  cp "$STAGE/latest.json" "$LOCAL/$VERSION/latest.json"
  cp "$STAGE/install.sh" "$LOCAL/install.sh"
  # The manifest last, once what it names is there.
  [[ ! -f "$STAGE/latest.json.sig" ]] || cp "$STAGE/latest.json.sig" "$LOCAL/latest.json.sig"
  cp "$STAGE/latest.json" "$LOCAL/latest.json.new"
  mv -f "$LOCAL/latest.json.new" "$LOCAL/latest.json"
  echo "published Claktory $VERSION to $LOCAL"
  echo "try it with: FACTORY_UPDATE_URL=$MANIFEST_URL"
fi
