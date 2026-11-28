#!/usr/bin/env bash
# Signs a release of the real Claktory so macOS trusts it: the app
# (Factory.app), the backend (factoryd), the apps host beside it
# (claktory-apps) and the supervisor's app (supervisor/Claktory Agents.app),
# with the Developer ID, the hardened runtime and a secure timestamp. Then it
# notarizes both with Apple and staples the ticket to the app.
# scripts/release.sh runs it on each release it builds; --dev releases stay
# signed ad hoc. The app's helpers (Contents/Helpers) are the signed backend,
# apps host and supervisor's app, copied in before the app is signed. No
# release carries a launchd daemon (#371's root helper was dropped, Fugo
# 24379): an app with one is refused.
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
# With the app's Developer ID provisioning profile (CLAKTORY_MAC_PROFILE, or
# ~/.appstoreconnect/claktory-mac.provisionprofile when it's there), the app
# also gets Keychain Sharing: the profile goes in as embedded.provisionprofile
# and the app's entitlements gain its application identifier, its team and
# one keychain group, TEAM.<the app's bundle id>, which macOS honours only
# with that profile (the data-protection keychain). The profile must be for
# exactly this app and team, signed for this identity's certificate, and
# unexpired. None of it changes the app's designated requirement, which
# the Keychain items it already has trust.
#
# --dry-run signs with the Developer ID (only the identity is needed) and
# checks the signatures, but sends nothing to Apple: it says what it would
# send. Try it on a copy, never on a published release.
#
# --notarize-only signs nothing: for a release the build service's signer has
# already signed (scripts/release-from-service.sh), where this Mac's
# account holds no Developer ID key. It checks the signatures as below
# (Developer ID, CLAKTORY_TEAM_ID's team, required, nothing open to
# injection), then notarizes and staples as below.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/lib.sh"

DIR="${1:-}"
DRY_RUN=false NOTARIZE_ONLY=false
case "${2:-}" in
  "") ;;
  --dry-run) DRY_RUN=true ;;
  --notarize-only) NOTARIZE_ONLY=true ;;
  *) DIR="" ;;
esac
if [[ -z "$DIR" || ! -f "$DIR/factoryd" ]]; then
  echo "usage: scripts/sign-release.sh RELEASE_DIR [--dry-run | --notarize-only]" >&2
  exit 2
fi
APP="$(find "$DIR" -maxdepth 1 -name '*.app' -type d | head -1)"
[[ -n "$APP" ]] || { echo "$DIR has no app" >&2; exit 2; }
SUPERVISOR="$DIR/supervisor/Claktory Agents.app"
[[ -d "$SUPERVISOR" ]] || SUPERVISOR=""
APPS_HOST="$DIR/claktory-apps"
[[ -f "$APPS_HOST" ]] || APPS_HOST=""
SPEECH="$DIR/claktory-speech"
[[ -f "$SPEECH" ]] || SPEECH=""
VIDEO="$DIR/claktory-video-preview"
[[ -f "$VIDEO" ]] || VIDEO=""
RG="$DIR/tools/rg"
[[ -f "$RG" ]] || RG=""
JQ="$DIR/tools/jq"
[[ -f "$JQ" ]] || JQ=""

# No root daemon ships (#371: the lid-closed helper was dropped for good).
if [[ -e "$APP/Contents/Library/LaunchDaemons" || -e "$APP/Contents/Helpers/claktory-lid-helper" ]]; then
  echo "$APP carries a launchd daemon or the old lid-closed helper; no release ships one" >&2
  exit 1
fi

KEY=false
notary_key && KEY=true
if $NOTARIZE_ONLY; then
  $KEY || { echo "no notarization key (~/.appstoreconnect/claktory.env, see above)" >&2; exit 1; }
  [[ -n "${CLAKTORY_TEAM_ID:-}" ]] || { echo "--notarize-only needs CLAKTORY_TEAM_ID, the team every piece must be signed by" >&2; exit 1; }
  # Whoever signed it, as the signature says: a Developer ID, of this team.
  IDENTITY="$(codesign -dvv "$DIR/factoryd" 2>&1 | sed -n 's/^Authority=\(Developer ID Application: .*\)$/\1/p' | head -1)"
  [[ -n "$IDENTITY" ]] || { echo "$DIR/factoryd isn't signed with a Developer ID" >&2; exit 1; }
  [[ "$IDENTITY" == *"($CLAKTORY_TEAM_ID)" ]] || { echo "$DIR isn't signed by team CLAKTORY_TEAM_ID" >&2; exit 1; }
  for code in "$APP" ${SUPERVISOR:+"$SUPERVISOR"} ${APPS_HOST:+"$APPS_HOST"} ${SPEECH:+"$SPEECH"} ${VIDEO:+"$VIDEO"} ${RG:+"$RG"} ${JQ:+"$JQ"}; do
    info="$(codesign -dvv "$code" 2>&1)"
    [[ "$(sed -n 's/^Authority=\(Developer ID Application: .*\)$/\1/p' <<<"$info" | head -1)" == "$IDENTITY" ]] ||
      { echo "${code#"$DIR"/} isn't signed by $IDENTITY" >&2; exit 1; }
    [[ "$(sed -n 's/^TeamIdentifier=//p' <<<"$info")" == "$CLAKTORY_TEAM_ID" ]] || { echo "${code#"$DIR"/} isn't signed by team CLAKTORY_TEAM_ID" >&2; exit 1; }
    [[ "$info" == *$'\n'Timestamp=* ]] || { echo "${code#"$DIR"/} has no secure timestamp" >&2; exit 1; }
    [[ "$info" =~ flags=[^$'\n']*runtime ]] || { echo "${code#"$DIR"/} isn't signed with the hardened runtime" >&2; exit 1; }
  done
else
  IDENTITY="$(developer_id_identity)"
fi
if ! $NOTARIZE_ONLY && ! $DRY_RUN && ! { [[ -n "$IDENTITY" ]] && $KEY; }; then
  if [[ -z "$IDENTITY" ]]; then
    echo "signed ad hoc: no Developer ID identity in the keychain"
  else
    echo "signed ad hoc: no notarization key (~/.appstoreconnect/claktory.env, see scripts/sign-release.sh)"
  fi
  # The helpers went into the app after Flutter signed it: seal them in
  # again, inside out, so the app verifies (the online installer checks).
  for helper in "$APP/Contents/Helpers/"*; do
    [[ -e "$helper" ]] || continue
    case "$(basename "$helper")" in
      claktory-browser-host) codesign --force --sign - --identifier com.claktory.browser-host "$helper" ;;
      claktory-video-preview) codesign --force --sign - --identifier dev.factory.claktory-video-preview "$helper" ;;
      tools)
        [[ ! -f "$helper/rg" ]] || codesign --force --sign - --identifier dev.factory.rg "$helper/rg"
        [[ ! -f "$helper/jq" ]] || codesign --force --sign - --identifier dev.factory.jq "$helper/jq"
        ;;
      *) codesign --force --sign - "$helper" ;;
    esac
  done
  codesign --force --sign - --preserve-metadata=entitlements,requirements,flags "$APP"
  codesign --verify --deep --strict "$APP"
  exit 0
fi
[[ -n "$IDENTITY" ]] || { echo "no Developer ID identity in the keychain to sign with" >&2; exit 1; }
if ! $NOTARIZE_ONLY; then

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
# The backend: a stable identifier (the linker's changes with every build),
# so its designated requirement, which keychain items trust, survives updates.
sign --identifier dev.factory.factoryd "$DIR/factoryd"
# The apps host compiles apps to machine code as it loads them, which the
# hardened runtime allows only with its entitlement (scripts/claktory-apps.entitlements).
if [[ -n "$APPS_HOST" ]]; then
  sign --identifier dev.factory.claktory-apps --entitlements "${SRC_ROOT:-$ROOT}/scripts/claktory-apps.entitlements" "$APPS_HOST"
  codesign --verify --strict "$APPS_HOST"
fi
# The speech host: no entitlements (its Metal library is embedded; no JIT).
if [[ -n "$SPEECH" ]]; then
  sign --identifier dev.factory.claktory-speech "$SPEECH"
  codesign --verify --strict "$SPEECH"
fi
# The video preview helper (#379): no entitlements (AVFoundation, one file in,
# one out; no network, no other programs).
if [[ -n "$VIDEO" ]]; then
  sign --identifier dev.factory.claktory-video-preview "$VIDEO"
  codesign --verify --strict "$VIDEO"
fi
# Agents' tools (ripgrep and jq, scripts/build-rg.sh and build-jq.sh): a
# stable identifier each, no entitlements.
if [[ -n "$RG" ]]; then
  sign --identifier dev.factory.rg "$RG"
  codesign --verify --strict "$RG"
fi
if [[ -n "$JQ" ]]; then
  sign --identifier dev.factory.jq "$JQ"
  codesign --verify --strict "$JQ"
fi
# The supervisor's app: its identity is its bundle's (dev.factory.supervisor),
# which a Developer Tools grant keeps across reinstalls.
if [[ -n "$SUPERVISOR" ]]; then
  sign "$SUPERVISOR"
  codesign --verify --strict --deep "$SUPERVISOR"
fi
# The helpers in the app (scripts/lib.sh add_helpers) are the release's own,
# just signed: the same bytes as the ones beside the app.
HELPERS="$APP/Contents/Helpers"
if [[ -d "$HELPERS" ]]; then
  cp "$DIR/factoryd" "$HELPERS/factoryd"
  if [[ -n "$APPS_HOST" ]]; then cp "$APPS_HOST" "$HELPERS/claktory-apps"; fi
  if [[ -n "$SPEECH" ]]; then cp "$SPEECH" "$HELPERS/claktory-speech"; fi
  if [[ -n "$VIDEO" ]]; then cp "$VIDEO" "$HELPERS/claktory-video-preview"; fi
  if [[ -n "$RG" ]]; then mkdir -p "$HELPERS/tools"; cp "$RG" "$HELPERS/tools/rg"; fi
  if [[ -n "$JQ" ]]; then mkdir -p "$HELPERS/tools"; cp "$JQ" "$HELPERS/tools/jq"; fi
  if [[ -n "$SUPERVISOR" ]]; then
    rm -rf "$HELPERS/Claktory Agents.app"
    ditto "$SUPERVISOR" "$HELPERS/Claktory Agents.app"
  fi
fi
# The browser helper (scripts/lib.sh add_browser_host), when the release
# has it: its fixed identifier is what the app's requirement for it names
# (with this team), and the helper checks the app the same way.
if [[ -f "$HELPERS/claktory-browser-host" ]]; then
  sign --identifier com.claktory.browser-host "$HELPERS/claktory-browser-host"
  codesign --verify --strict "$HELPERS/claktory-browser-host"
fi
ENTITLEMENTS="${SRC_ROOT:-$ROOT}/app/macos/Runner/Release.entitlements"
PROFILE="${CLAKTORY_MAC_PROFILE:-}"
[[ -n "$PROFILE" || ! -f "$HOME/.appstoreconnect/claktory-mac.provisionprofile" ]] || PROFILE="$HOME/.appstoreconnect/claktory-mac.provisionprofile"
if [[ -n "$PROFILE" ]]; then
  PROFILE_WORK="$(mktemp -d)"
  security cms -D -i "$PROFILE" >"$PROFILE_WORK/profile.plist" 2>/dev/null || { echo "$PROFILE isn't a provisioning profile" >&2; exit 1; }
  BUNDLE_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Contents/Info.plist")"
  TEAM="$(/usr/libexec/PlistBuddy -c 'Print :TeamIdentifier:0' "$PROFILE_WORK/profile.plist")"
  [[ "$IDENTITY" == *"($TEAM)" ]] || { echo "the Mac profile is for another team than $IDENTITY" >&2; exit 1; }
  [[ "$(/usr/libexec/PlistBuddy -c 'Print :Entitlements:com.apple.application-identifier' "$PROFILE_WORK/profile.plist")" == "$TEAM.$BUNDLE_ID" ]] ||
    { echo "the Mac profile isn't for $BUNDLE_ID" >&2; exit 1; }
  /usr/libexec/PlistBuddy -c 'Print :ProvisionsAllDevices' "$PROFILE_WORK/profile.plist" 2>/dev/null | grep -qx true ||
    { echo "the Mac profile isn't a Developer ID one (it doesn't provision all devices)" >&2; exit 1; }
  EXPIRES="$(/usr/libexec/PlistBuddy -c 'Print :ExpirationDate' "$PROFILE_WORK/profile.plist")"
  (( $(date -j -f '%a %b %d %T %Z %Y' "$EXPIRES" +%s) > $(date +%s) + 86400 )) || { echo "the Mac profile expires $EXPIRES: make a new one" >&2; exit 1; }
  # Signed for this identity's certificate.
  security find-certificate -c "${IDENTITY#Developer ID Application: }" -Z 2>/dev/null | sed -n 's/^SHA-1 hash: //p' | head -1 >"$PROFILE_WORK/ours"
  found=false
  for ((i = 0; ; i++)); do
    /usr/libexec/PlistBuddy -c "Print :DeveloperCertificates:$i" "$PROFILE_WORK/profile.plist" >"$PROFILE_WORK/cert" 2>/dev/null || break
    [[ "$(openssl x509 -inform DER -in "$PROFILE_WORK/cert" -noout -fingerprint -sha1 2>/dev/null | sed 's/.*=//; s/://g')" == "$(cat "$PROFILE_WORK/ours")" ]] && found=true
  done
  $found || { echo "the Mac profile isn't signed for $IDENTITY's certificate" >&2; exit 1; }
  cp "$PROFILE" "$APP/Contents/embedded.provisionprofile"
  cp "$ENTITLEMENTS" "$PROFILE_WORK/app.entitlements"
  /usr/libexec/PlistBuddy \
    -c "Add :com.apple.application-identifier string $TEAM.$BUNDLE_ID" \
    -c "Add :com.apple.developer.team-identifier string $TEAM" \
    -c 'Add :keychain-access-groups array' \
    -c "Add :keychain-access-groups: string $TEAM.$BUNDLE_ID" \
    "$PROFILE_WORK/app.entitlements" >/dev/null
  ENTITLEMENTS="$PROFILE_WORK/app.entitlements"
  echo "with the Mac profile: Keychain Sharing for $BUNDLE_ID (expires $EXPIRES)"
fi
sign --entitlements "$ENTITLEMENTS" "$APP"
[[ -z "${PROFILE_WORK:-}" ]] || rm -rf "$PROFILE_WORK"
fi

codesign --verify --strict --deep "$APP"
codesign --verify --strict "$DIR/factoryd"
# Nothing the backend trusts by its signature may be open to injection
# (#546 A, Fugo via Giorno 43915): the app on app.sock is trusted by its code
# signature, which holds only while no library or debugger can get into it.
# So no signed code in the release may carry an entitlement that lets one in.
INJECTABLE="com.apple.security.cs.allow-dyld-environment-variables com.apple.security.cs.disable-library-validation com.apple.security.get-task-allow"
while IFS= read -r -d '' code; do
  { codesign -d --entitlements - --xml "$code" 2>/dev/null || true; } | python3 -c '
import plistlib, sys
raw = sys.stdin.buffer.read()
ents = plistlib.loads(raw) if raw.strip() else {}
bad = " and ".join(k for k in sys.argv[2].split() if ents.get(k) is True)
if bad:
    sys.exit(sys.argv[1] + " carries " + bad + ": code with it can be injected into, so the backend could not trust it; not releasing")
' "${code#"$DIR"/}" "$INJECTABLE" || exit 1
done < <(
  { printf '%s\0' "$APP" "$DIR/factoryd" ${SUPERVISOR:+"$SUPERVISOR"} ${APPS_HOST:+"$APPS_HOST"} ${SPEECH:+"$SPEECH"} ${VIDEO:+"$VIDEO"}
    find "$APP/Contents" \( -name '*.app' -o -name '*.framework' -o -name '*.dylib' -o -type f -perm -u+x \) -print0 2>/dev/null; }
)
echo "no signed code in it allows injection (library validation, dyld variables, get-task-allow)"
if $NOTARIZE_ONLY; then echo "signed with $IDENTITY (by the build service)"; else echo "signed with $IDENTITY"; fi

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
# One archive with both: notarytool takes a zip, and a bare binary only in one.
mkdir "$STAGE/release"
ditto "$APP" "$STAGE/release/$(basename "$APP")"
cp "$DIR/factoryd" "$STAGE/release/factoryd"
if [[ -n "$APPS_HOST" ]]; then cp "$APPS_HOST" "$STAGE/release/claktory-apps"; fi
if [[ -n "$SPEECH" ]]; then cp "$SPEECH" "$STAGE/release/claktory-speech"; fi
if [[ -n "$VIDEO" ]]; then cp "$VIDEO" "$STAGE/release/claktory-video-preview"; fi
if [[ -n "$RG" ]]; then mkdir -p "$STAGE/release/tools"; cp "$RG" "$STAGE/release/tools/rg"; fi
if [[ -n "$JQ" ]]; then mkdir -p "$STAGE/release/tools"; cp "$JQ" "$STAGE/release/tools/jq"; fi
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
