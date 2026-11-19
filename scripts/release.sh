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
# A release for the real Factory needs its entries in docs/CHANGELOG.md,
# stamped with its version by scripts/changelog.sh stamp (and committed); it
# builds that version. Each release carries the changelog, which the app shows.
#
#   scripts/release.sh         for the real Factory (~/.factory, Factory.app)
#   scripts/release.sh --dev   for the dev Factory (~/.factory-dev, Factory Dev)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/lib.sh"

case "${1:-}" in
  "") HOME_DIR="$HOME/.factory"; APP_NAME="Factory.app"; DEFINES=() ;;
  --dev) HOME_DIR="$HOME/.factory-dev"; APP_NAME="Factory Dev.app"; DEFINES=(--dart-define=FACTORY_PORT=7421 --dart-define=FACTORY_DEV=true) ;;
  *) echo "usage: scripts/release.sh [--dev]" >&2; exit 2 ;;
esac
RELEASES="$HOME_DIR/releases"
mkdir -p "$RELEASES"
if [[ "$APP_NAME" == "Factory.app" ]]; then
  # The version the changelog's newest entries are stamped with.
  FACTORY_VERSION="$("$ROOT/scripts/changelog.sh" pending "$RELEASES")"
else
  FACTORY_VERSION="$(stamp_version "$RELEASES")"
fi
export FACTORY_VERSION

FINGERPRINT="$(backend_fingerprint)"
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
(cd "$ROOT/app" && flutter build macos --release --dart-define=FACTORY_VERSION="$FACTORY_VERSION" ${DEFINES[@]+"${DEFINES[@]}"})

partial="$RELEASES/$FACTORY_VERSION.partial"
rm -rf "$partial"
mkdir -p "$partial"
cp "$BACKEND" "$partial/factoryd"
echo "$BACKEND_VERSION" >"$partial/backend-version"
echo "$FINGERPRINT" >"$partial/backend-fingerprint"
if [[ "$APP_NAME" == "Factory.app" ]]; then
  # What's unreleased stays out of a real release.
  awk '/^## /{skip = ($0 == "## Unreleased")} !skip' "$ROOT/docs/CHANGELOG.md" >"$partial/CHANGELOG.md"
else
  cp "$ROOT/docs/CHANGELOG.md" "$partial/CHANGELOG.md"
fi
take_build "$ROOT/app/build/macos/Build/Products/Release/Factory.app" "$partial/$APP_NAME"
if [[ "$APP_NAME" == "Factory Dev.app" ]]; then
  make_dev_app "$partial/$APP_NAME" Release.entitlements
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
echo "published Factory $FACTORY_VERSION to $RELEASES"
