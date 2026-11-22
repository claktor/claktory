#!/usr/bin/env bash
# Builds Claktory for iPhone, for the Simulator, and with --run opens it there.
#
#   scripts/ios.sh [--dev] [--run] [--target FILE] [--device UDID]
#                  [--define KEY=VALUE]…
#
# --dev builds Claktory Dev (bundle id com.claktory.app.dev, its own icon and
# keychain), as scripts/dev.sh does for the Mac; without it, Claktory
# (com.claktory.app). --run installs it in the booted Simulator, booting an
# iPhone first when none is, and opens it; with --device, in that Simulator
# instead (its UDID, from `xcrun simctl list devices`), so each agent keeps
# to its own.
#
# --target builds another entry point than lib/main.dart, such as an
# integration test, and --define passes it a --dart-define. That's how to
# run an integration test as Claktory Dev, paired as it already is: `flutter
# test integration_test/… -d` would build com.claktory.app instead.
#
#   scripts/ios.sh --dev --run --device <UDID> \
#     --target integration_test/mobile_navigation_test.dart \
#     --define REPORT=/tmp/navigation.txt
#
# The phone app has no backend of its own: it pairs with a Claktory through
# the private tunnel. Signing for a device, TestFlight and push aren't set up.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dev=false
run=false
target=""
device=""
defines=()
usage="usage: scripts/ios.sh [--dev] [--run] [--target FILE] [--device UDID] [--define KEY=VALUE]…"
while [ $# -gt 0 ]; do
  case "$1" in
    --dev) dev=true ;;
    --run) run=true ;;
    --target) target="${2:?$usage}"; shift ;;
    --device) device="${2:?$usage}"; shift ;;
    --define) defines+=("--dart-define=${2:?$usage}"); shift ;;
    *) echo "$usage" >&2; exit 2 ;;
  esac
  shift
done
build=(flutter build ios --simulator --debug)
if [ -n "$target" ]; then build+=("--target=$target"); fi

cd "$root/app"
if $dev; then build+=(--dart-define=FACTORY_DEV=true); fi
"${build[@]}" ${defines[@]+"${defines[@]}"}
app="$root/app/build/ios/iphonesimulator/Runner.app"
id=com.claktory.app

if $dev; then
  id=com.claktory.app.dev
  plist="$app/Info.plist"
  /usr/libexec/PlistBuddy \
    -c "Set :CFBundleIdentifier $id" \
    -c "Set :CFBundleName Claktory Dev" \
    -c "Set :CFBundleDisplayName Claktory Dev" \
    -c "Set :CFBundleURLTypes:0:CFBundleURLSchemes:0 claktory-dev" "$plist"
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
  # Its notification extension's id must start with the app's.
  appex="$app/PlugIns/NotificationService.appex"
  if [ -d "$appex" ]; then
    /usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $id.NotificationService" "$appex/Info.plist"
    codesign --force --sign - --preserve-metadata=entitlements "$appex"
  fi
  codesign --force --sign - --preserve-metadata=entitlements "$app"
fi
echo "Built $app ($id)"

if $run && [ -n "$device" ]; then
  if ! xcrun simctl list devices booted | grep -q "$device"; then
    xcrun simctl boot "$device"
  fi
  # The on-screen keyboard even with the Mac's keyboard attached, as on a
  # phone: else the Simulator hides it, and what's typed covers nothing.
  xcrun simctl spawn "$device" defaults write com.apple.keyboard.preferences \
    AutomaticMinimizationEnabled -bool false
  xcrun simctl install "$device" "$app"
  xcrun simctl launch "$device" "$id"
elif $run; then
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
