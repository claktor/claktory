#!/usr/bin/env bash
# Makes the disk image people download to install Claktory on a Mac:
# Claktory-<version>.dmg, holding the release's app as "Claktory.app" (the
# bundle and its id stay the same; only the name people see changes) and a
# link to /Applications to drag it onto. The app carries its helpers
# (Contents/Helpers), so on first launch it sets Claktory up by itself.
# With the Developer ID and the notarization key (scripts/sign-release.sh says
# where), the image is signed, notarized and stapled too; without them it's
# left unsigned, for trying out, and says so. scripts/publish.sh runs it.
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

IDENTITY=""
if ! $UNSIGNED; then
  IDENTITY="$(developer_id_identity)"
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

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
mkdir "$STAGE/Claktory"
ditto "$APP" "$STAGE/Claktory/Claktory.app"
ln -s /Applications "$STAGE/Claktory/Applications"
rm -f "$DMG"
# Compressed with lzfse (ULFO, macOS 10.11 and later), HFS+ for any Mac.
hdiutil create -quiet -volname Claktory -srcfolder "$STAGE/Claktory" -fs HFS+ -format ULFO "$DMG"
hdiutil verify -quiet "$DMG"

if $UNSIGNED; then
  echo "unsigned (for trying out): $DMG" >&2
  echo "$DMG"
  exit 0
fi

codesign --force --timestamp --sign "$IDENTITY" "$DMG"
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
