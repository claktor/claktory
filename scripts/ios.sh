#!/usr/bin/env bash
# Builds Claktory for iPhone, for the Simulator, and with --run opens it there.
#
#   scripts/ios.sh [--dev] [--run]
#
# --dev builds Claktory Dev (bundle id com.claktory.app.dev, its own icon and
# keychain), as scripts/dev.sh does for the Mac; without it, Claktory
# (com.claktory.app). --run installs it in the booted Simulator, booting an
# iPhone first when none is, and opens it.
#
# The phone app has no backend of its own: it pairs with a Claktory through
# the private tunnel. Signing for a device, TestFlight and push aren't set up.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dev=false
run=false
for arg in "$@"; do
  case "$arg" in
    --dev) dev=true ;;
    --run) run=true ;;
    *) echo "usage: scripts/ios.sh [--dev] [--run]" >&2; exit 2 ;;
  esac
done

cd "$root/app"
if $dev; then
  flutter build ios --simulator --debug --dart-define=FACTORY_DEV=true
else
  flutter build ios --simulator --debug
fi
app="$root/app/build/ios/iphonesimulator/Runner.app"
id=com.claktory.app

if $dev; then
  id=com.claktory.app.dev
  plist="$app/Info.plist"
  /usr/libexec/PlistBuddy \
    -c "Set :CFBundleIdentifier $id" \
    -c "Set :CFBundleName Claktory Dev" \
    -c "Set :CFBundleDisplayName Claktory Dev" "$plist"
  # The dev icon (scripts/icons.swift), as loose files: without
  # CFBundleIconName, iOS takes the icon from them instead of the catalog.
  cp "$root"/app/ios/dev/*.png "$app/"
  for key in CFBundleIcons CFBundleIcons~ipad; do
    /usr/libexec/PlistBuddy -c "Delete :$key:CFBundlePrimaryIcon:CFBundleIconName" "$plist" 2>/dev/null || true
  done
  /usr/libexec/PlistBuddy \
    -c "Delete :CFBundleIcons~ipad:CFBundlePrimaryIcon:CFBundleIconFiles" \
    -c "Add :CFBundleIcons~ipad:CFBundlePrimaryIcon:CFBundleIconFiles array" \
    -c "Add :CFBundleIcons~ipad:CFBundlePrimaryIcon:CFBundleIconFiles:0 string AppIcon76x76" \
    -c "Add :CFBundleIcons~ipad:CFBundlePrimaryIcon:CFBundleIconFiles:1 string AppIcon83.5x83.5" "$plist"
  codesign --force --sign - "$app"
fi
echo "Built $app ($id)"

if $run; then
  if ! xcrun simctl list devices booted | grep -q Booted; then
    device="$(xcrun simctl list devices available | sed -n 's/^ *\(iPhone [^(]*\) (\([0-9A-F-]*\)).*/\2/p' | head -1)"
    xcrun simctl boot "$device"
    # Like the Mac app, the phone follows light or dark mode; the Simulator
    # starts light, so match this Mac's.
    if [ "$(defaults read -g AppleInterfaceStyle 2>/dev/null)" = Dark ]; then
      xcrun simctl ui booted appearance dark
    fi
  fi
  xcrun simctl install booted "$app"
  xcrun simctl launch booted "$id"
fi
