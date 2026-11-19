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
#   install.sh    the online installer (scripts/install-online.sh), for
#                 curl -fsSL https://github.com/claktor/claktory/releases/latest/download/install.sh | bash
#
#   scripts/publish.sh [--repo OWNER/NAME] [--notes TEXT] [--no-linux]
#       to GitHub Releases, claktor/claktory unless --repo names another.
#       Before building it checks that main is checked out and clean, and
#       that no secret is anywhere in its history (scripts/check-secrets.sh,
#       which needs gitleaks); after building, before uploading, it pushes
#       main to the repository (scripts/push-source.sh: never forced, except
#       once over a new repository's README-only first commit), so the
#       source of every release is public with it.
#   scripts/publish.sh --local DIR [--dev]
#       to a channel in a folder, to try it out; --dev releases for the dev
#       Factory. Point a backend at it with FACTORY_UPDATE_URL=file://DIR/latest.json.
#   --no-linux  without the Linux builds (Linux backends then keep the release
#               they have)
#
# Agents: leave publishing to GitHub to Coordinator and the person.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REPO="" LOCAL="" DEV=() NOTES="" LINUX=(x86_64 aarch64)
DEFAULT_REPO="claktor/claktory"
usage() {
  echo "usage: scripts/publish.sh [--repo OWNER/NAME] [--notes TEXT] [--no-linux] | --local DIR [--dev] [--no-linux]" >&2
  exit 2
}
while (($#)); do
  case "$1" in
    --repo) REPO="${2:-}"; shift 2 || usage ;;
    --local) LOCAL="${2:-}"; shift 2 || usage ;;
    --notes) NOTES="${2:-}"; shift 2 || usage ;;
    --dev) DEV=(--dev); shift ;;
    --no-linux) LINUX=(); shift ;;
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

FACTORY_UPDATE_URL="$MANIFEST_URL" "$ROOT/scripts/release.sh" ${DEV[@]+"${DEV[@]}"}

if [[ ${#DEV[@]} -eq 0 ]]; then
  HOME_DIR="$HOME/.factory" APP_NAME="Factory.app"
else
  HOME_DIR="$HOME/.factory-dev" APP_NAME="Factory Dev.app"
fi
VERSION="$(cat "$HOME_DIR/releases/latest")"
RELEASE="$HOME_DIR/releases/$VERSION"
NAME="factory-$VERSION-macos.tar.gz"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
# The license travels with every copy (PolyForm Shield).
COPYFILE_DISABLE=1 tar --no-xattrs -czf "$STAGE/$NAME" \
  -C "$RELEASE" "$APP_NAME" factoryd backend-version CHANGELOG.md \
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
  cp "$RELEASE/backend-version" "$RELEASE/CHANGELOG.md" "$build/"
  linux_name="factory-$VERSION-linux-$arch.tar.gz"
  COPYFILE_DISABLE=1 tar --no-xattrs -czf "$STAGE/$linux_name" \
    -C "$build" factoryd backend-version CHANGELOG.md \
    -C "$ROOT" LICENSE.md \
    -C "$ROOT/scripts" install-linux.sh
  FILES+=("$STAGE/$linux_name")
  # One line per build: the online installer reads it with sed, on machines without jq.
  [[ -n "$LINUX_JSON" ]] && LINUX_JSON+=$',\n'
  LINUX_JSON+="    \"$arch\": {\"url\": \"$(url_of "$linux_name")\", \"sha256\": \"$(shasum -a 256 "$STAGE/$linux_name" | cut -d' ' -f1)\", \"size\": $(stat -f %z "$STAGE/$linux_name")}"
done

# The top-level url is the Mac's, as every backend so far reads it; Linux
# backends read theirs under "linux".
cat >"$STAGE/latest.json" <<JSON
{
  "version": "$VERSION",
  "backend": "$(cat "$RELEASE/backend-version")",
  "url": "$URL",
  "sha256": "$(shasum -a 256 "$STAGE/$NAME" | cut -d' ' -f1)",
  "size": $(stat -f %z "$STAGE/$NAME"),
  "min_macos": "$MIN_MACOS",
  "archs": [$ARCHS],
  "linux": {
$LINUX_JSON
  }
}
JSON
sed "s|^MANIFEST_URL=\"\"\$|MANIFEST_URL=\"$MANIFEST_URL\"|" "$ROOT/scripts/install-online.sh" >"$STAGE/install.sh"
grep -q "^MANIFEST_URL=\"$MANIFEST_URL\"\$" "$STAGE/install.sh" || { echo "couldn't fill in install.sh" >&2; exit 1; }

if [[ -n "$REPO" ]]; then
  # The source first: a release is never public without its source.
  "$ROOT/scripts/push-source.sh" "https://github.com/$REPO.git" --expect "$SOURCE"
  gh release create "$VERSION" "${FILES[@]}" "$STAGE/latest.json" "$STAGE/install.sh" \
    --repo "$REPO" --target "$SOURCE" --title "Claktory $VERSION" --notes "${NOTES:-Claktory $VERSION}"
  echo "published Factory $VERSION to $BASE"
  echo "install on another Mac, or a Linux machine, with:"
  echo "  curl -fsSL $BASE/latest/download/install.sh | bash"
else
  mkdir -p "$LOCAL/$VERSION"
  cp "${FILES[@]}" "$LOCAL/$VERSION/"
  cp "$STAGE/install.sh" "$LOCAL/install.sh"
  # The manifest last, once what it names is there.
  cp "$STAGE/latest.json" "$LOCAL/latest.json.new"
  mv -f "$LOCAL/latest.json.new" "$LOCAL/latest.json"
  echo "published Factory $VERSION to $LOCAL"
  echo "try it with: FACTORY_UPDATE_URL=$MANIFEST_URL"
fi
