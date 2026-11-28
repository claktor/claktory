#!/usr/bin/env bash
# Makes the disk image people download to install Claktory on a Mac:
# Claktory-<version>.dmg, holding the release's app as "Claktory.app" (the
# bundle and its id stay the same; only the name people see changes) and a
# link to /Applications to drag it onto, on a background with an arrow from
# one to the other (scripts/dmg: the window's layout for dmgbuild, which
# writes it without Finder, and its pinned requirements, installed once into
# ~/Library/Caches/claktory-dmgbuild; the background's drawn by
# scripts/dmg-background.swift). The app carries its helpers
# (Contents/Helpers), so on first launch it sets Claktory up by itself.
# With the Developer ID and the notarization key (scripts/sign-release.sh says
# where), the image is signed, notarized and stapled too; without them it's
# left unsigned, for trying out, and says so. On a Mac with the build service,
# whose signer alone holds the Developer ID, a release that came from
# it (its build-request file, scripts/release-from-service.sh) has its image
# signed there (scripts/build-request.sh dmg), and notarized here.
# scripts/publish.sh runs it.
#
#   scripts/make-dmg.sh RELEASE_DIR OUT_DIR [--unsigned]
#
# Writes OUT_DIR/Claktory-<version>.dmg and prints its path.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/lib.sh"

RELEASE="${1:-}" OUT="${2:-}" UNSIGNED=false
[[ "${3:-}" == --unsigned ]] && UNSIGNED=true
if [[ -z "$RELEASE" || -z "$OUT" || ! -d "$RELEASE/Factory.app" ]]; then
  echo "usage: scripts/make-dmg.sh RELEASE_DIR OUT_DIR [--unsigned] (a release of the real Claktory, with Factory.app)" >&2
  exit 2
fi
VERSION="$(basename "$RELEASE")"
APP="$RELEASE/Factory.app"
[[ -x "$APP/Contents/Helpers/factoryd" ]] || { echo "$APP has no helpers (Contents/Helpers/factoryd): it can't set Claktory up from a .dmg; build it again" >&2; exit 1; }
mkdir -p "$OUT"
DMG="$(cd "$OUT" && pwd)/Claktory-$VERSION.dmg"

IDENTITY="" SERVICE=""
if ! $UNSIGNED; then
  IDENTITY="$(developer_id_identity)"
  if [[ -z "$IDENTITY" && -f "$RELEASE/build-request" ]]; then
    SERVICE="$(cat "$RELEASE/build-request")"
    IDENTITY="the build service's ($SERVICE)"
  fi
  [[ -n "$IDENTITY" ]] && notary_key || {
    echo "make-dmg: no Developer ID identity or notarization key (see scripts/sign-release.sh); use --unsigned for an image to try out" >&2
    exit 1
  }
  # The app inside must already be notarized, with its ticket stapled.
  xcrun stapler validate -q "$APP" && spctl --assess --type execute "$APP" 2>/dev/null || {
    echo "make-dmg: $APP isn't notarized; a published image only holds a notarized app" >&2
    exit 1
  }
fi

# dmgbuild, in a virtual environment of its own, installed again when its
# pinned requirements change.
VENV="${XDG_CACHE_HOME:-$HOME/Library/Caches}/claktory-dmgbuild"
if ! cmp -s "$ROOT/scripts/dmg/requirements.txt" "$VENV/requirements.txt"; then
  rm -rf "$VENV"
  python3 -m venv "$VENV"
  "$VENV/bin/pip" install --quiet --disable-pip-version-check --require-hashes --only-binary :all: \
    -r "$ROOT/scripts/dmg/requirements.txt"
  cp "$ROOT/scripts/dmg/requirements.txt" "$VENV/requirements.txt"
fi

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
rm -f "$DMG"
# Compressed with lzfse (ULFO, macOS 10.11 and later), HFS+ for any Mac.
"$VENV/bin/dmgbuild" -s "$ROOT/scripts/dmg/settings.py" -D app="$APP" -D dir="$ROOT/scripts/dmg" Claktory "$DMG" >/dev/null
hdiutil verify -quiet "$DMG"

if $UNSIGNED; then
  echo "unsigned (for trying out): $DMG" >&2
  echo "$DMG"
  exit 0
fi

if [[ -n "$SERVICE" ]]; then
  SIGNED="$("$ROOT/scripts/build-request.sh" dmg "$SERVICE" "$DMG")"
  cp "$SIGNED" "$DMG"
  SIGNER="$(codesign -dvv "$DMG" 2>&1 | sed -n 's/^Authority=\(Developer ID Application: .*\)$/\1/p' | head -1)"
  [[ -n "$SIGNER" ]] || { echo "make-dmg: the build service's image isn't signed with a Developer ID" >&2; exit 1; }
  [[ "$SIGNER" == "$(codesign -dvv "$APP" 2>&1 | sed -n 's/^Authority=\(Developer ID Application: .*\)$/\1/p' | head -1)" ]] ||
    { echo "make-dmg: the image isn't signed by whoever signed the app" >&2; exit 1; }
else
  codesign --force --timestamp --sign "$IDENTITY" "$DMG"
fi
codesign --verify --strict "$DMG"
echo "notarizing $(basename "$DMG") with Apple (usually a few minutes)…" >&2
xcrun notarytool submit "$DMG" \
  --key "$ASC_KEY_PATH" --key-id "$ASC_KEY_ID" --issuer "$ASC_ISSUER_ID" \
  --wait --output-format json >"$STAGE/result.json" || true
STATUS="$(plutil -extract status raw "$STAGE/result.json" 2>/dev/null)" || STATUS="no answer"
if [[ "$STATUS" != Accepted ]]; then
  echo "make-dmg: notarization failed: $STATUS" >&2
  if ID="$(plutil -extract id raw "$STAGE/result.json" 2>/dev/null)"; then
    xcrun notarytool log "$ID" --key "$ASC_KEY_PATH" --key-id "$ASC_KEY_ID" --issuer "$ASC_ISSUER_ID" >&2 || true
  fi
  exit 1
fi
xcrun stapler staple -q "$DMG"
xcrun stapler validate -q "$DMG"
# What Gatekeeper asks of a downloaded image before it opens it.
spctl --assess --type open --context context:primary-signature "$DMG"
echo "signed, notarized and stapled: $DMG" >&2
echo "$DMG"
