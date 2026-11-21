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
#                 curl -fsSL https://github.com/claktor/claktory/releases/latest/download/install.sh | bash
#
#   scripts/publish.sh [--repo OWNER/NAME] [--notes TEXT] [--no-linux]
#       to GitHub Releases, claktor/claktory unless --repo names another.
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
#   --no-linux  without the Linux builds (Linux backends then keep the release
#               they have)
#   --built     publishes the release already built (the newest in
#               ~/.factory/releases, or ~/.factory-dev's with --dev) instead
#               of building one: so a release can be built and signed first,
#               tried on this Mac, and then published exactly as tried. Build
#               it from this commit with the channel's address stamped in:
#                 FACTORY_UPDATE_URL=https://github.com/claktor/claktory/releases/latest/download/latest.json scripts/release.sh
#               It refuses a release built from another commit or for
#               another channel.
#   --adhoc     allows a Mac app that isn't notarized (signed ad hoc): building
#               one (scripts/release.sh --adhoc) and publishing it to GitHub;
#               without it, both are refused.
#
# Agents: leave publishing to GitHub to Coordinator and the person.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REPO="" LOCAL="" DEV=() NOTES="" LINUX=(x86_64 aarch64) BUILT=false ADHOC=false
DEFAULT_REPO="claktor/claktory"
usage() {
  echo "usage: scripts/publish.sh [--repo OWNER/NAME] [--notes TEXT] [--no-linux] [--built] [--adhoc] | --local DIR [--dev] [--no-linux] [--built]" >&2
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
    *) usage ;;
  esac
done
if [[ -n "$REPO" && -n "$LOCAL" ]]; then usage; fi
[[ -n "$LOCAL" ]] || REPO="${REPO:-$DEFAULT_REPO}"

if [[ -n "$REPO" ]]; then
  [[ "$REPO" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || usage
  [[ ${#DEV[@]} -eq 0 ]] || { echo "--dev releases only go to a --local channel" >&2; exit 2; }
  # Before building: is the repository there, and can gh reach it?
  gh repo view "$REPO" --json name >/dev/null
  # Before building: the source to push is main, clean, without secrets. The
  # dry run checks all of it (and what the repository holds) and pushes nothing.
  SOURCE="$(git -C "$ROOT" rev-parse HEAD)"
  "$ROOT/scripts/push-source.sh" "https://github.com/$REPO.git" --expect "$SOURCE" --dry-run
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

if $BUILT; then
  VERSION="$(cat "$HOME_DIR/releases/latest" 2>/dev/null)" || { echo "no release in $HOME_DIR/releases to publish" >&2; exit 1; }
  RELEASE="$HOME_DIR/releases/$VERSION"
  HEAD="$(git -C "$ROOT" rev-parse HEAD)"
  BUILT_FROM="$(cat "$RELEASE/source" 2>/dev/null || true)"
  if [[ "$BUILT_FROM" != "$HEAD" ]]; then
    # Only files that neither build nor ship in a release may have changed
    # since (a secret-check exception, say): then what was built is still
    # this commit's release, and the source pushed with it is HEAD.
    NOT_SHIPPED='^(\.gitleaks\.toml|AGENTS\.md|scripts/(publish|check-secrets|push-source|testflight)\.sh)$'
    CHANGED="$(git -C "$ROOT" diff --name-only "$BUILT_FROM" "$HEAD" 2>/dev/null)" &&
      git -C "$ROOT" merge-base --is-ancestor "$BUILT_FROM" "$HEAD" &&
      { [[ -z "$CHANGED" ]] || ! grep -Eqv "$NOT_SHIPPED" <<<"$CHANGED"; } || {
      echo "$VERSION was built from ${BUILT_FROM:-an unrecorded commit}, and this checkout ($HEAD) changed what goes into a release since; build it again from here" >&2
      exit 1
    }
    echo "$VERSION was built from ${BUILT_FROM:0:7}; since then only $(tr '\n' ' ' <<<"$CHANGED")changed, which a release doesn't contain"
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
  echo "publishing $VERSION as built ($RELEASE)"
else
  # Before building: would it be notarized?
  if [[ -n "$REPO" ]] && ! $ADHOC; then
    source "$ROOT/scripts/lib.sh"
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
  require_notarized "$RELEASE/$APP_NAME"
fi
NAME="factory-$VERSION-macos.tar.gz"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
# The license travels with every copy (PolyForm Shield).
# The supervisor's app, in releases that have one (scripts/lib.sh make_supervisor_app).
SUPERVISOR_DIR=()
[[ -d "$RELEASE/supervisor" ]] && SUPERVISOR_DIR=(supervisor)
COPYFILE_DISABLE=1 tar --no-xattrs -czf "$STAGE/$NAME" \
  -C "$RELEASE" "$APP_NAME" factoryd claktory-apps claktory-speech backend-version CHANGELOG.md ${SUPERVISOR_DIR[@]+"${SUPERVISOR_DIR[@]}"} \
  -C "$ROOT" LICENSE.md \
  -C "$ROOT/scripts" install-release.sh

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
  FACTORY_VERSION="$BACKEND_VERSION" FACTORY_UPDATE_URL="$MANIFEST_URL" "$ROOT/scripts/build-linux.sh" "$arch" "$build/factoryd"
  "$ROOT/scripts/build-linux.sh" "$arch" "$build/claktory-apps" claktory-apps
  "$ROOT/scripts/build-linux.sh" "$arch" "$build/claktory-speech" claktory-speech
  cp "$RELEASE/backend-version" "$RELEASE/CHANGELOG.md" "$build/"
  linux_name="factory-$VERSION-linux-$arch.tar.gz"
  COPYFILE_DISABLE=1 tar --no-xattrs -czf "$STAGE/$linux_name" \
    -C "$build" factoryd claktory-apps claktory-speech backend-version CHANGELOG.md \
    -C "$ROOT" LICENSE.md \
    -C "$ROOT/scripts" install-linux.sh
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

# The top-level url is the Mac's, as every backend so far reads it; Linux
# backends read theirs under "linux".
cat >"$STAGE/latest.json" <<JSON
{
  "version": "$VERSION",
  "released_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
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
# The publisher's signature over latest.json (G1, agreed with Security):
# ed25519 over the file's exact bytes, a one-line sidecar
# "claktory-release-1:BASE64" that factoryd verifies against its pinned
# public keys on https channels. The key lives only in the vault
# (CLAKTORY_SIGNING_KEY, a file): run publish.sh with that secret. Signed
# whenever the key is here; a real publish without it warns loudly until
# factoryd's verify half ships, then it will refuse instead. Old backends
# never fetch the sidecar, so it changes nothing for them.
SIGNING_KEY_ID="claktory-release-1"
if [[ -n "${CLAKTORY_SIGNING_KEY:-}" ]]; then
  openssl version | grep -q "^OpenSSL 3" || { echo "signing needs OpenSSL 3 (brew install openssl@3); this openssl is: $(openssl version)" >&2; exit 1; }
  openssl pkeyutl -sign -inkey "$CLAKTORY_SIGNING_KEY" -rawin -in "$STAGE/latest.json" -out "$STAGE/latest.sig.bin"
  printf '%s:%s\n' "$SIGNING_KEY_ID" "$(base64 <"$STAGE/latest.sig.bin" | tr -d '\n')" >"$STAGE/latest.json.sig"
  # Self-check: the signature verifies against the key's own public half,
  # so a wrongly pasted vault value is caught here, never shipped. The
  # public key (not a secret) goes in the log.
  openssl pkey -in "$CLAKTORY_SIGNING_KEY" -pubout -out "$STAGE/latest.pub.pem"
  openssl pkeyutl -verify -pubin -inkey "$STAGE/latest.pub.pem" -rawin -in "$STAGE/latest.json" -sigfile "$STAGE/latest.sig.bin" >/dev/null ||
    { echo "the fresh signature doesn't verify: is CLAKTORY_SIGNING_KEY the release signing key?" >&2; exit 1; }
  echo "signed latest.json as $SIGNING_KEY_ID (public key $(openssl pkey -in "$STAGE/latest.pub.pem" -pubin -pubout -outform DER | tail -c 32 | base64))"
  rm -f "$STAGE/latest.sig.bin" "$STAGE/latest.pub.pem"
  FILES+=("$STAGE/latest.json.sig")
elif [[ -n "$REPO" ]]; then
  echo "publish.sh: refusing to publish unsigned: factoryd verifies latest.json against the pinned release keys, so an unsigned publish would stop every update. Run publish.sh with the vault's CLAKTORY_SIGNING_KEY (the Claktory release signing key)." >&2
  exit 1
fi

sed "s|^MANIFEST_URL=\"\"\$|MANIFEST_URL=\"$MANIFEST_URL\"|" "$ROOT/scripts/install-online.sh" >"$STAGE/install.sh"
grep -q "^MANIFEST_URL=\"$MANIFEST_URL\"\$" "$STAGE/install.sh" || { echo "couldn't fill in install.sh" >&2; exit 1; }

if [[ -n "$REPO" ]]; then
  # The source first: a release is never public without its source.
  "$ROOT/scripts/push-source.sh" "https://github.com/$REPO.git" --version "$VERSION" --expect "$SOURCE"
  # Its tag is the release's public commit, which push-source.sh left as the branch public.
  PUBLIC_COMMIT="$(git -C "$ROOT" rev-parse refs/heads/public)"
  # The release page says what the version brings: the changelog section,
  # which is the public commit's own message body (public_commit in lib.sh).
  if [[ -z "$NOTES" ]]; then
    NOTES="$(git -C "$ROOT" log -1 --format=%b refs/heads/public)"
    [[ -n "$NOTES" ]] || NOTES="Claktory $VERSION"
  fi
  gh release create "$VERSION" "${FILES[@]}" "$STAGE/latest.json" "$STAGE/install.sh" \
    --repo "$REPO" --target "$PUBLIC_COMMIT" --title "Claktory $VERSION" --notes "$NOTES"
  echo "published Claktory $VERSION to $BASE"
  echo "install on another Mac, or a Linux machine, with:"
  echo "  curl -fsSL $BASE/latest/download/install.sh | bash"
else
  mkdir -p "$LOCAL/$VERSION"
  cp "${FILES[@]}" "$LOCAL/$VERSION/"
  cp "$STAGE/install.sh" "$LOCAL/install.sh"
  # The manifest last, once what it names is there.
  [[ ! -f "$STAGE/latest.json.sig" ]] || cp "$STAGE/latest.json.sig" "$LOCAL/latest.json.sig"
  cp "$STAGE/latest.json" "$LOCAL/latest.json.new"
  mv -f "$LOCAL/latest.json.new" "$LOCAL/latest.json"
  echo "published Claktory $VERSION to $LOCAL"
  echo "try it with: FACTORY_UPDATE_URL=$MANIFEST_URL"
fi
