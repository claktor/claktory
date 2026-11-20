#!/usr/bin/env bash
# Signs a release of the real Claktory so macOS trusts it: the app
# (Factory.app), the backend (factoryd) and the supervisor's app
# (supervisor/Claktory Agents.app),
# with the Developer ID, the hardened runtime and a secure timestamp. Then it
# notarizes both with Apple and staples the ticket to the app.
# scripts/release.sh runs it on each release it builds; --dev releases stay
# signed ad hoc.
#
#   scripts/sign-release.sh RELEASE_DIR [--dry-run]
#
# It needs:
#   - a "Developer ID Application" identity in the keychain (the first one,
#     or CLAKTORY_SIGN_IDENTITY; with CLAKTORY_TEAM_ID, that team's), and
#   - the App Store Connect API key for notarization, the same settings as
#     scripts/testflight.sh: ASC_KEY_ID, ASC_ISSUER_ID and the .p8 (ASC_KEY_PATH,
#     or ~/.appstoreconnect/private_keys/AuthKey_<ASC_KEY_ID>.p8), from the
#     environment or ~/.appstoreconnect/claktory.env.
# Without both, it leaves the release signed ad hoc, as built, and says so.
#
# --dry-run signs with the Developer ID (only the identity is needed) and
# checks the signatures, but sends nothing to Apple: it says what it would
# send. Try it on a copy, never on a published release.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/lib.sh"

DIR="${1:-}"
DRY_RUN=false
case "${2:-}" in
  "") ;;
  --dry-run) DRY_RUN=true ;;
  *) DIR="" ;;
esac
if [[ -z "$DIR" || ! -f "$DIR/factoryd" ]]; then
  echo "usage: scripts/sign-release.sh RELEASE_DIR [--dry-run]" >&2
  exit 2
fi
APP="$(find "$DIR" -maxdepth 1 -name '*.app' -type d | head -1)"
[[ -n "$APP" ]] || { echo "$DIR has no app" >&2; exit 2; }
SUPERVISOR="$DIR/supervisor/Claktory Agents.app"
[[ -d "$SUPERVISOR" ]] || SUPERVISOR=""

IDENTITY="$(developer_id_identity)"
KEY=false
notary_key && KEY=true
if ! $DRY_RUN && ! { [[ -n "$IDENTITY" ]] && $KEY; }; then
  if [[ -z "$IDENTITY" ]]; then
    echo "signed ad hoc: no Developer ID identity in the keychain"
  else
    echo "signed ad hoc: no notarization key (~/.appstoreconnect/claktory.env, see scripts/sign-release.sh)"
  fi
  exit 0
fi
[[ -n "$IDENTITY" ]] || { echo "no Developer ID identity in the keychain to sign with" >&2; exit 1; }

# The hardened runtime, which notarization requires, with a secure timestamp.
sign() {
  codesign --force --timestamp --options runtime --sign "$IDENTITY" "$@"
}

# Inside out: what the app holds, then the app. The app's entitlements are
# the release build's own (network client); with the hardened runtime, the
# Dart code (compiled ahead of time in a release) needs no JIT.
find "$APP/Contents" -name '*.dylib' -type f -print0 | while IFS= read -r -d '' library; do sign "$library"; done
for framework in "$APP"/Contents/Frameworks/*.framework; do
  if [[ -d "$framework" ]]; then sign "$framework"; fi
done
sign --entitlements "$ROOT/app/macos/Runner/Release.entitlements" "$APP"
# The backend: a stable identifier (the linker's changes with every build),
# so its designated requirement, which keychain items trust, survives updates.
sign --identifier dev.factory.factoryd "$DIR/factoryd"
# The supervisor's app: its identity is its bundle's (dev.factory.supervisor),
# which a Developer Tools grant keeps across reinstalls.
if [[ -n "$SUPERVISOR" ]]; then
  sign "$SUPERVISOR"
  codesign --verify --strict --deep "$SUPERVISOR"
fi

codesign --verify --strict --deep "$APP"
codesign --verify --strict "$DIR/factoryd"
echo "signed with $IDENTITY"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
# One archive with both: notarytool takes a zip, and a bare binary only in one.
mkdir "$STAGE/release"
ditto "$APP" "$STAGE/release/$(basename "$APP")"
cp "$DIR/factoryd" "$STAGE/release/factoryd"
if [[ -n "$SUPERVISOR" ]]; then
  ditto "$SUPERVISOR" "$STAGE/release/supervisor/$(basename "$SUPERVISOR")"
fi
ditto -c -k --keepParent "$STAGE/release" "$STAGE/release.zip"

if $DRY_RUN; then
  echo "dry run: would notarize $(du -h "$STAGE/release.zip" | cut -f1 | tr -d ' ') ($(cd "$STAGE/release" && ls | tr '\n' ' ' | sed 's/ $//')) with xcrun notarytool submit --wait, then staple the apps"
  exit 0
fi

echo "notarizing with Apple (usually a few minutes)…"
xcrun notarytool submit "$STAGE/release.zip" \
  --key "$ASC_KEY_PATH" --key-id "$ASC_KEY_ID" --issuer "$ASC_ISSUER_ID" \
  --wait --output-format json >"$STAGE/result.json" || true
# plutil says why it can't read something on stdout: keep only what it read.
STATUS="$(plutil -extract status raw "$STAGE/result.json" 2>/dev/null)" || STATUS="no answer"
if [[ "$STATUS" != Accepted ]]; then
  echo "notarization failed: $STATUS" >&2
  if ID="$(plutil -extract id raw "$STAGE/result.json" 2>/dev/null)"; then
    xcrun notarytool log "$ID" --key "$ASC_KEY_PATH" --key-id "$ASC_KEY_ID" --issuer "$ASC_ISSUER_ID" >&2 || true
  fi
  exit 1
fi
# The ticket goes in the app, so it opens offline too; a bare binary can't
# hold one, and macOS looks factoryd's up online when it needs it.
for bundle in "$APP" ${SUPERVISOR:+"$SUPERVISOR"}; do
  xcrun stapler staple -q "$bundle"
  xcrun stapler validate -q "$bundle"
  spctl --assess --type execute "$bundle"
done
echo "notarized and stapled"
