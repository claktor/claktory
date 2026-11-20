#!/usr/bin/env bash
# Publishes this checkout as a new version of Factory without touching the
# running one. It builds the backend and app into <home>/releases/<version>/
# and names that version in <home>/releases/latest. Factory notices within a
# second, and its app shows an Update banner; the person decides when.
#
# When the backend's source hasn't changed since the last release, the
# release reuses that backend, so updating to it only replaces the app and
# never interrupts the agents. A backend that fails to start after an update
# is undone by the supervisor (crates/factoryd/src/update.rs).
#
# A release for the real Factory is signed with the Developer ID and notarized
# when this Mac has the identity and the notarization key, and signed ad hoc
# otherwise (scripts/sign-release.sh says which).
#
# A release for the real Factory needs its entries in docs/CHANGELOG.md,
# stamped with its version by scripts/changelog.sh stamp (and committed); it
# builds that version. Each release carries the changelog, which the app shows.
#
#   scripts/release.sh         for the real Factory (~/.factory, Factory.app)
#   scripts/release.sh --dev   for the dev Factory (~/.factory-dev, Factory Dev)
#   scripts/release.sh --out DIR
#                              a release for the real Factory, built and
#                              signed exactly the same, into DIR instead of
#                              ~/.factory/releases: a rehearsal (of signing
#                              and notarizing, say) that no running Claktory
#                              sees, so it shows no Update banner.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/lib.sh"

usage() { echo "usage: scripts/release.sh [--dev | --out DIR]" >&2; exit 2; }
case "${1:-}" in
  "") RELEASES="$HOME/.factory/releases"; APP_NAME="Factory.app"; DEFINES=() ;;
  --dev) RELEASES="$HOME/.factory-dev/releases"; APP_NAME="Factory Dev.app"; DEFINES=(--dart-define=FACTORY_PORT=7421 --dart-define=FACTORY_DEV=true) ;;
  --out)
    [[ -n "${2:-}" ]] || usage
    RELEASES="$(mkdir -p "$2" && cd "$2" && pwd)"
    case "$RELEASES/" in
      "$HOME/.factory/"* | "$HOME/.factory-dev/"*) echo "--out is for a folder outside ~/.factory and ~/.factory-dev" >&2; exit 2 ;;
    esac
    APP_NAME="Factory.app"; DEFINES=()
    ;;
  *) usage ;;
esac
if [[ "$APP_NAME" == "Factory.app" ]]; then
  # Built only from what's committed: a release is never someone's work in
  # progress. The real one only from main's commit (on main, or a checkout
  # whose branch is exactly main).
  [[ -z "$(git -C "$ROOT" status --porcelain)" ]] || {
    echo "the checkout has uncommitted or untracked changes; a release is built from a clean tree only:" >&2
    git -C "$ROOT" status --short >&2
    exit 1
  }
  if [[ "$RELEASES" == "$HOME/.factory/releases" && "$(git -C "$ROOT" rev-parse HEAD)" != "$(git -C "$ROOT" rev-parse main)" ]]; then
    echo "a release is built from main's commit only; this checkout is at $(git -C "$ROOT" rev-parse --short HEAD), main at $(git -C "$ROOT" rev-parse --short main)" >&2
    exit 1
  fi
fi
mkdir -p "$RELEASES"
if [[ "$APP_NAME" == "Factory.app" ]]; then
  # The version the changelog's newest entries are stamped with.
  FACTORY_VERSION="$("$ROOT/scripts/changelog.sh" pending "$RELEASES")"
else
  FACTORY_VERSION="$(stamp_version "$RELEASES")"
fi
export FACTORY_VERSION

FINGERPRINT="$(backend_fingerprint)"
if [[ "$APP_NAME" == "Factory.app" ]]; then
  # Signing with a Developer ID makes a different backend: the first signed
  # release ships one even when the source didn't change.
  SIGNING="$(signing_mode)"
  if [[ "$SIGNING" != adhoc ]]; then
    FINGERPRINT="$(printf '%s\nsigning: %s\n' "$FINGERPRINT" "$SIGNING" | shasum -a 256 | cut -d' ' -f1)"
  fi
fi
PREVIOUS=""
if [[ -f "$RELEASES/latest" ]]; then
  PREVIOUS="$RELEASES/$(cat "$RELEASES/latest")"
fi
if [[ -n "$PREVIOUS" && -f "$PREVIOUS/factoryd" && -f "$PREVIOUS/backend-version" && "$(cat "$PREVIOUS/backend-fingerprint" 2>/dev/null)" == "$FINGERPRINT" ]]; then
  BACKEND="$PREVIOUS/factoryd"
  BACKEND_VERSION="$(cat "$PREVIOUS/backend-version")"
  echo "backend unchanged since $BACKEND_VERSION; this release only updates the app"
else
  cargo build --manifest-path "$ROOT/Cargo.toml" --release -p factoryd
  BACKEND="$ROOT/target/release/factoryd"
  BACKEND_VERSION="$FACTORY_VERSION"
fi
# Finder's version too (Info.plist takes numbers only: v0.1.3 is 0.1.3).
BUNDLE_VERSION="$(sed -n 's/^v\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\).*/\1/p' <<<"$FACTORY_VERSION")"
BUNDLE_VERSION="${BUNDLE_VERSION:-0.0.0}"
(cd "$ROOT/app" && flutter build macos --release --build-name "$BUNDLE_VERSION" --build-number "$BUNDLE_VERSION" --dart-define=FACTORY_VERSION="$FACTORY_VERSION" ${DEFINES[@]+"${DEFINES[@]}"})

partial="$RELEASES/$FACTORY_VERSION.partial"
rm -rf "$partial"
mkdir -p "$partial"
cp "$BACKEND" "$partial/factoryd"
echo "$BACKEND_VERSION" >"$partial/backend-version"
echo "$FINGERPRINT" >"$partial/backend-fingerprint"
# What it was built from, for scripts/publish.sh --built: the commit, and the
# channel stamped into its backend (none unless FACTORY_UPDATE_URL was set).
git -C "$ROOT" rev-parse HEAD >"$partial/source"
echo "${FACTORY_UPDATE_URL:-}" >"$partial/update-url"
if [[ "$APP_NAME" == "Factory.app" ]]; then
  # What's unreleased stays out of a real release.
  awk '/^## /{skip = ($0 == "## Unreleased")} !skip' "$ROOT/docs/CHANGELOG.md" >"$partial/CHANGELOG.md"
else
  cp "$ROOT/docs/CHANGELOG.md" "$partial/CHANGELOG.md"
fi
take_build "$ROOT/app/build/macos/Build/Products/Release/Factory.app" "$partial/$APP_NAME"
if [[ "$APP_NAME" == "Factory Dev.app" ]]; then
  make_dev_app "$partial/$APP_NAME" Release.entitlements
else
  # The supervisor as an app (Developer Tools needs one); installers put it
  # in place, updates never do.
  make_supervisor_app "$partial"
  # Developer ID and notarized when this Mac has what that takes; ad hoc
  # otherwise, as dev releases always are.
  "$ROOT/scripts/sign-release.sh" "$partial"
fi
mv "$partial" "$RELEASES/$FACTORY_VERSION"
# Named only once complete; the rename makes the change atomic.
echo "$FACTORY_VERSION" >"$RELEASES/latest.new"
mv -f "$RELEASES/latest.new" "$RELEASES/latest"

# Keep the three newest, by version (v0.1.10 is newer than v0.1.9).
ls -1 "$RELEASES" | grep -v '^latest' | while read -r name; do
  printf '%s\t%s\n' "$(version_key "$name")" "$name"
done | sort -r | cut -f2 | tail -n +4 | while read -r old; do
  rm -rf "${RELEASES:?}/$old"
done
echo "published Claktory $FACTORY_VERSION to $RELEASES"
