#!/usr/bin/env bash
# Builds Claktory for iPhone (com.claktory.app) for the App Store and
# uploads it to App Store Connect, where it shows up in TestFlight.
#
#   scripts/testflight.sh [--no-upload]
#
# --no-upload archives and signs it, and leaves the .ipa in
# app/build/ios/testflight/, without sending it anywhere.
#
# It signs automatically (Xcode makes or renews the certificate and profile
# it needs) and uploads with an App Store Connect API key. It needs, from
# the environment or from ~/.appstoreconnect/claktory.env (KEY=value lines,
# kept outside the repository):
#
#   CLAKTORY_TEAM_ID   the Apple Developer team ID, ten characters
#   ASC_KEY_ID         the API key's ID, ten characters
#   ASC_ISSUER_ID      the API key's issuer ID, a UUID
#   ASC_KEY_PATH       optional: the key's .p8; by default
#                      ~/.appstoreconnect/private_keys/AuthKey_<ASC_KEY_ID>.p8
#
# The version is the latest released one in docs/CHANGELOG.md (v0.1.3 is
# 0.1.3); the build number is the time it's built, so every upload is new.
# Nothing here is committed, and no secret is printed.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
upload=true
for arg in "$@"; do
  case "$arg" in
    --no-upload) upload=false ;;
    *) echo "usage: scripts/testflight.sh [--no-upload]" >&2; exit 2 ;;
  esac
done

settings="$HOME/.appstoreconnect/claktory.env"
if [ -f "$settings" ]; then
  # Only the names it knows, and never echoed.
  while IFS='=' read -r key value; do
    case "$key" in
      CLAKTORY_TEAM_ID | ASC_KEY_ID | ASC_ISSUER_ID | ASC_KEY_PATH)
        if [ -z "${!key:-}" ]; then
          value="${value%\"}"; value="${value#\"}"
          export "$key=$value"
        fi
        ;;
    esac
  done <"$settings"
fi

missing=()
[ -n "${CLAKTORY_TEAM_ID:-}" ] || missing+=("CLAKTORY_TEAM_ID (the Apple Developer team ID)")
[ -n "${ASC_KEY_ID:-}" ] || missing+=("ASC_KEY_ID (the App Store Connect API key's ID)")
[ -n "${ASC_ISSUER_ID:-}" ] || missing+=("ASC_ISSUER_ID (the API key's issuer ID)")
key_path="${ASC_KEY_PATH:-$HOME/.appstoreconnect/private_keys/AuthKey_${ASC_KEY_ID:-KEYID}.p8}"
if [ ${#missing[@]} -gt 0 ] || [ ! -f "$key_path" ]; then
  {
    echo "Can't build for TestFlight yet. Missing:"
    for item in "${missing[@]}"; do echo "  - $item"; done
    [ -f "$key_path" ] || echo "  - the API key file, $key_path"
    echo "Set them in the environment or in $settings (KEY=value lines);"
    echo "the header of scripts/testflight.sh says what each is."
  } >&2
  exit 1
fi

# The newest Xcode in /Applications (Xcode.app, Xcode-26.app…), or
# CLAKTORY_XCODE: App Store Connect takes only apps built with a recent SDK,
# and the newest can sit beside the one everyone else builds with.
if [ -z "${DEVELOPER_DIR:-}" ]; then
  xcode="${CLAKTORY_XCODE:-}"
  if [ -z "$xcode" ]; then
    best=""
    for candidate in /Applications/Xcode*.app; do
      [ -d "$candidate/Contents/Developer" ] || continue
      v="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$candidate/Contents/Info.plist" 2>/dev/null || echo 0)"
      key="$(printf '%05d%05d%05d' $(echo "$v.0.0" | tr '.' ' ' | cut -d' ' -f1-3))"
      if [ -z "$best" ] || [[ "$key" > "$best" ]]; then best="$key"; xcode="$candidate"; fi
    done
  fi
  if [ -n "$xcode" ]; then export DEVELOPER_DIR="$xcode/Contents/Developer"; fi
fi
echo "With $(xcodebuild -version | head -1) (${DEVELOPER_DIR:-$(xcode-select -p)})"

version="$(sed -n 's/^## v\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\)$/\1/p' "$root/docs/CHANGELOG.md" | head -1)"
if [ -z "$version" ]; then
  echo "No released version in docs/CHANGELOG.md to build." >&2
  exit 1
fi
build="$(date -u +%Y%m%d%H%M)"
out="$root/app/build/ios/testflight"
archive="$out/Claktory.xcarchive"
rm -rf "$out"
mkdir -p "$out"

auth=(
  -allowProvisioningUpdates
  -authenticationKeyPath "$key_path"
  -authenticationKeyID "$ASC_KEY_ID"
  -authenticationKeyIssuerID "$ASC_ISSUER_ID"
)

echo "Building Claktory $version ($build) for iPhone…"
cd "$root/app"
# Flutter's half (the Dart code, its assets and the tunnel's Rust library),
# then Xcode's archive of it. The bundle id, com.claktory.app, comes from
# the project: set on the command line it would reach the plugins' resource
# bundles too, and App Store Connect refuses two bundles with one id.
# The archive is left unsigned, and exporting it signs it for the App Store:
# signing the archive itself would use the project's development identity,
# whose profile needs a registered device (the team may have none), while
# the export's distribution certificate and App Store profile need none.
flutter build ios --release --config-only --build-name "$version" --build-number "$build"
cd ios
xcodebuild archive -quiet \
  -workspace Runner.xcworkspace -scheme Runner -configuration Release \
  -destination 'generic/platform=iOS' -archivePath "$archive" \
  DEVELOPMENT_TEAM="$CLAKTORY_TEAM_ID" CODE_SIGN_STYLE=Automatic \
  CODE_SIGNING_ALLOWED=NO

destination=export
$upload && destination=upload
options="$out/ExportOptions.plist"
cat >"$options" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>method</key><string>app-store-connect</string>
  <key>destination</key><string>$destination</string>
  <key>teamID</key><string>$CLAKTORY_TEAM_ID</string>
  <key>signingStyle</key><string>automatic</string>
  <key>uploadSymbols</key><true/>
  <key>manageAppVersionAndBuildNumber</key><false/>
</dict>
</plist>
EOF

xcodebuild -exportArchive -quiet \
  -archivePath "$archive" -exportOptionsPlist "$options" -exportPath "$out" \
  "${auth[@]}"

if $upload; then
  echo "Uploaded Claktory $version ($build). It shows in TestFlight once App Store Connect has processed it, usually within half an hour."
else
  echo "Built $out (Claktory $version, build $build); not uploaded."
fi
