#!/usr/bin/env bash
# Builds Claktory for iPhone and iPad (com.claktory.app, one universal app)
# for the App Store and uploads it to App Store Connect, where it shows up
# in TestFlight and can be submitted for review.
#
#   scripts/testflight.sh [--no-upload] [--source COMMIT]
#
# --no-upload archives and signs it, and leaves the .ipa in
# app/build/ios/testflight/, without sending it anywhere.
#
# Which devices it's for is the project's (TARGETED_DEVICE_FAMILY 1,2 and
# the iPad orientations in Info.plist since 6e042e1c; --source of an older
# commit builds that commit's iPhone-only app). An app for iPad must carry
# all four iPad orientations, which iPad multitasking requires (App Store
# Connect refuses it otherwise, error 90474): checked before uploading.
#
# Before anything is uploaded, scripts/ios-privacy-scan.py refuses an app
# whose binaries use a required-reason API their privacy manifest doesn't
# declare (ITMS-91053), or a private or iOS-unavailable symbol (guideline
# 2.5.1): App Review's automatic refusals, caught here.
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
#
# --source builds that commit, which must be on main: a release's own
# source (the `source` in its folder in ~/.factory/releases), so the iPhone
# app of v0.1.16 is v0.1.16's code even once main has moved on.
#
# Otherwise it builds the checkout's commit (HEAD), so the checkout must be clean, and
# builds it in the same private folder as scripts/release.sh, never in the
# checkout itself: Dart's snapshot keeps the absolute path of a file Flutter
# generates (dart-lang/sdk#55282), so a build in a home folder would put
# that path in the app. Before anything is signed or uploaded, no binary in
# the app may carry this Mac's home folder, user or host name
# (check_no_traces in scripts/lib.sh).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$root/scripts/lib.sh"
upload=true
from=""
usage() { echo "usage: scripts/testflight.sh [--no-upload] [--source COMMIT]" >&2; exit 2; }
while [ $# -gt 0 ]; do
  case "$1" in
    --no-upload) upload=false; shift ;;
    --ipad) echo "--ipad is gone: every build is for iPhone and iPad, as the project says (6e042e1c)." >&2; exit 2 ;;
    --source) [ $# -ge 2 ] || usage; from="$2"; shift 2 ;;
    *) usage ;;
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

if [ -n "$from" ]; then
  commit="$(git -C "$root" rev-parse --verify -q "$from^{commit}")" &&
    git -C "$root" merge-base --is-ancestor "$commit" main || {
    echo "--source $from isn't a commit on main." >&2
    exit 1
  }
else
  if [ -n "$(git -C "$root" status --porcelain --untracked-files=no)" ]; then
    echo "The checkout has uncommitted changes: commit them, since only the commit is built." >&2
    exit 1
  fi
  commit="$(git -C "$root" rev-parse HEAD)"
fi
src="$(release_tree "$root" "$commit")"
signing="$(mktemp -d)"
trap 'rm -rf "$signing"; release_unlock' EXIT
release_inputs
release_pub_get "$src/app"

version="$(sed -n 's/^## v\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\)$/\1/p' "$src/docs/CHANGELOG.md" | head -1)"
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

echo "Building Claktory $version ($build) from ${commit:0:10} in ${src}…"
cd "$src/app"
# Flutter's half (the Dart code, its assets and the tunnel's Rust library),
# then Xcode's archive of it. The bundle id, com.claktory.app, comes from
# the project: set on the command line it would reach the plugins' resource
# bundles too, and App Store Connect refuses two bundles with one id.
# The archive is left unsigned, and exporting it signs it for the App Store:
# signing the archive itself would use the project's development identity,
# whose profile needs a registered device (the team may have none), while
# the export's distribution certificate and App Store profile need none.
# Its build folder is its own, inside the export (fresh every run): the
# shared DerivedData is also every other build's on this Mac, and other
# Xcode versions'.
# FACTORY_VERSION stamps the version the app shows and checks its backend
# against (app/lib/update.dart); unstamped, the app reads "dev".
flutter build ios --release --config-only --build-name "$version" --build-number "$build" --dart-define=FACTORY_VERSION="v$version"
cd ios
xcodebuild archive -quiet \
  -workspace Runner.xcworkspace -scheme Runner -configuration Release \
  -destination 'generic/platform=iOS' -archivePath "$archive" \
  -derivedDataPath "$src/app/build/ios/testflight-derived" \
  DEVELOPMENT_TEAM="$CLAKTORY_TEAM_ID" CODE_SIGN_STYLE=Automatic \
  CODE_SIGNING_ALLOWED=NO

app_dir="$archive/Products/Applications/Runner.app"
# Unsigned, the archive expands $(AppIdentifierPrefix) to nothing in the
# Info.plists: v0.1.16 shipped a push keychain group without its team
# prefix, the key never saved and the phone never registered for
# notifications. Fill it in from the sources before anything is signed.
python3 "$root/scripts/ios-build-vars.py" fill "$app_dir" "$src/app/ios" "$CLAKTORY_TEAM_ID"
plist="$app_dir/Info.plist"
devices="iPhone"
if /usr/libexec/PlistBuddy -c 'Print :UIDeviceFamily' "$plist" | grep -qE '^[[:space:]]*2$'; then
  devices="iPhone and iPad"
  [ "$(/usr/libexec/PlistBuddy -c 'Print :UISupportedInterfaceOrientations~ipad' "$plist" 2>/dev/null | grep -c UIInterfaceOrientation)" = 4 ] ||
    { echo "Not uploading: an app for iPad needs all four iPad orientations (UISupportedInterfaceOrientations~ipad), or App Store Connect refuses it (90474)." >&2; exit 1; }
fi
echo "Built for $devices."

# Every binary the app embeds must be loadable by the phone's dyld. A stale
# build cache can slip an old dylib in: the tunnel's Rust library, linked
# while an older Xcode was selected, reached TestFlight on 2026-11-21 and
# failed every dlopen on the phone with "mis-aligned LINKEDIT string pool"
# (the Simulator doesn't enforce it). So check what dyld checks, before
# anything is signed or uploaded: 8-aligned symbol and string tables, and
# the modern LC_BUILD_VERSION stamp (its absence means an old toolchain
# linked the binary).
problems=""
for binary in "$app_dir/Runner" "$app_dir"/Frameworks/*.framework/*; do
  [ -f "$binary" ] || continue
  file -b "$binary" | grep -q Mach-O || continue
  found="$(otool -l "$binary" | python3 -c '
import re, sys
text = sys.stdin.read()
bad = []
symtab = re.search(r"cmd LC_SYMTAB\n\s*cmdsize \d+\n\s*symoff (\d+)\n\s*nsyms \d+\n\s*stroff (\d+)", text)
if symtab:
    for name, off in (("symbol table", int(symtab.group(1))), ("string pool", int(symtab.group(2)))):
        if off % 8:
            bad.append(f"its {name} (offset {off}) is not 8-byte aligned, which the phone refuses")
if "cmd LC_BUILD_VERSION" not in text:
    bad.append("it has no LC_BUILD_VERSION: an old toolchain linked it")
print("; ".join(bad))
')"
  # And dyld's own reader: it refuses exactly what the phone would.
  if ! dyld="$(xcrun dyld_info -platform "$binary" 2>&1)"; then
    dyld="$(printf '%s' "$dyld" | grep -v "^$binary" | tail -1)"
    found="${found:+$found; }dyld_info: ${dyld:-it refuses the file}"
  fi
  [ -z "$found" ] || problems="$problems
  - ${binary#"$app_dir"/}: $found"
done
if [ -n "$problems" ]; then
  {
    echo "Not uploading: the phone's dyld would refuse what the app embeds:$problems"
    echo "A stale cache usually did it: delete app/.dart_tool/hooks_runner and app/build/ios, check xcode-select, and build again."
  } >&2
  exit 1
fi

# The app, its frameworks and its extensions carry no trace of this Mac.
check_no_traces "$app_dir"
# And every Mach-O in it loads the way the strictest device dyld demands.
python3 "$root/scripts/check-macho.py" "$app_dir" ||
  { echo "Not uploading: a device's dyld would refuse what's above." >&2; exit 1; }

# The export signs with the entitlements it finds on the archived code, and
# an unsigned archive has none: the app would reach TestFlight without push,
# time-sensitive notifications or its keychain groups (it did on 2026-11-22),
# and automatic signing would never turn Push on for the App IDs. So sign
# the archive ad hoc with the project's entitlements, its build variables
# filled in, and an App Store build's aps-environment; the export re-signs
# all of it with the distribution certificate and keeps them.
# The push keys' keychain group, the Release build's (app/ios/Flutter/
# Release.xcconfig, CLAKTORY_PUSH_GROUP): where the shipped app keeps them.
push_group=com.claktory.app.notifications
entitlements() { # SOURCE BUNDLE_ID OUT
  sed -e "s/\$(AppIdentifierPrefix)/$CLAKTORY_TEAM_ID./g" -e "s/\$(PRODUCT_BUNDLE_IDENTIFIER)/$2/g" \
    -e "s/\$(CLAKTORY_PUSH_GROUP)/$push_group/g" "$1" >"$3"
  if /usr/libexec/PlistBuddy -c 'Print :aps-environment' "$3" >/dev/null 2>&1; then
    /usr/libexec/PlistBuddy -c 'Set :aps-environment production' "$3"
  fi
}
for framework in "$app_dir"/Frameworks/*; do
  [ -e "$framework" ] || continue
  codesign --force --sign - "$framework"
done
for appex in "$app_dir"/PlugIns/*.appex; do
  [ -e "$appex" ] || continue
  name="$(basename "$appex" .appex)"
  entitlements "$src/app/ios/$name/$name.entitlements" \
    "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$appex/Info.plist")" "$signing/$name.entitlements"
  codesign --force --sign - --entitlements "$signing/$name.entitlements" "$appex"
done
entitlements "$src/app/ios/Runner/Runner.entitlements" com.claktory.app "$signing/Runner.entitlements"
codesign --force --sign - --entitlements "$signing/Runner.entitlements" "$app_dir"
# Nothing unexpanded, and every keychain group the app names is one it's
# entitled to, with the team prefix.
python3 "$root/scripts/ios-build-vars.py" check "$app_dir" "$CLAKTORY_TEAM_ID" ||
  { echo "Not uploading: the app's build variables aren't filled in (above)." >&2; exit 1; }
# No undeclared required-reason API, no private symbol (App Review's
# automatic refusals).
python3 "$root/scripts/ios-privacy-scan.py" "$app_dir" ||
  { echo "Not uploading: App Review would refuse this build (above)." >&2; exit 1; }

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

# Xcode packages the .ipa with /usr/bin/rsync (Apple's openrsync), which
# starts its other half as the first `rsync` on PATH: Homebrew's rsync 3.x
# there refuses openrsync's options ("--extended-attributes: unknown
# option", EXPORT FAILED, v0.1.25's first upload). Apple's own goes first.
mkdir "$signing/rsync" && ln -s /usr/bin/rsync "$signing/rsync/rsync"
PATH="$signing/rsync:$PATH" xcodebuild -exportArchive -quiet \
  -archivePath "$archive" -exportOptionsPlist "$options" -exportPath "$out" \
  "${auth[@]}"

# Uploading exports straight to App Store Connect and leaves no .ipa here;
# --no-upload does, and gets the same check on what would be sent.
if ! $upload; then
  payload="$signing/payload"
  mkdir "$payload"
  unzip -q "$out"/*.ipa -d "$payload"
  # The app only: the .ipa's Symbols/ go to Apple and name build paths.
  check_no_traces "$payload/Payload"
  # And as the distribution signature left it.
  python3 "$root/scripts/ios-build-vars.py" check "$payload/Payload/Runner.app" "$CLAKTORY_TEAM_ID"
  python3 "$root/scripts/ios-privacy-scan.py" "$payload/Payload/Runner.app" >/dev/null
fi
if $upload; then
  echo "Uploaded Claktory $version ($build) for $devices. It shows in TestFlight once App Store Connect has processed it, usually within half an hour."
else
  echo "Built $out (Claktory $version, build $build, for $devices); not uploaded."
fi
