#!/usr/bin/env bash
# Runs the agent characters' performance gate (benchmark/characters.dart) in
# profile mode as "Factory Bench", a separate app with its own bundle id that
# talks to no backend, and prints the results.
#
#   app/benchmark/run.sh             with a 5-minute soak at the end
#   BENCH_SOAK_MINUTES=1 app/benchmark/run.sh
#   BENCH_ONLY=idle,thinking app/benchmark/run.sh   only those scenes
#
# The window opens in the background, as big as the screen allows, and quits
# by itself when done (about 6 minutes plus the soak).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
source "$ROOT/scripts/lib.sh"
APP="$ROOT/app/build/Factory Bench.app"
APP_ID="dev.factory.app.bench"
OUT="/tmp/factory-characters-bench.json"

(cd "$ROOT/app" && flutter build macos --profile -t benchmark/characters.dart \
  --dart-define=BENCH_SOAK_MINUTES="${BENCH_SOAK_MINUTES:-5}" \
  --dart-define=BENCH_ONLY="${BENCH_ONLY:-}")
take_build "$ROOT/app/build/macos/Build/Products/Profile/Factory.app" "$APP"
plist="$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $APP_ID" -c "Set :CFBundleName Factory Bench" "$plist"
/usr/libexec/PlistBuddy -c "Delete :CFBundleDisplayName" "$plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :CFBundleDisplayName string Factory Bench" "$plist"
# No sandbox: it writes its results to /tmp, and reads its window frame below.
codesign --force --sign - "$APP"

# As big as the screen allows: the window opens with this frame.
defaults write "$APP_ID" "NSWindow Frame Factory main window" "0 0 3400 1600 0 0 3840 1600 "
rm -f "$OUT"
# -g: in the background, never taking focus. -W: until it quits.
open -g -W --env BENCH_OUT="$OUT" "$APP"
cat "$OUT"
