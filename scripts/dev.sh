#!/usr/bin/env bash
# Builds and runs the dev Factory: `factoryd --dev` (port 7421, data in
# ~/.factory-dev) under its supervisor, as the real one runs, and the
# "Factory Dev" app connected to it. The real Factory
# (port 7420, ~/.factory, Factory.app) keeps running untouched.
#
#   scripts/dev.sh           backend, then app
#   scripts/dev.sh backend   rebuild and restart only the backend
#   scripts/dev.sh app       rebuild and relaunch only the app
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/lib.sh"
DEV_HOME="$HOME/.factory-dev"
# Stamped like releases, so the app can tell when a newer release is published.
FACTORY_VERSION="$(stamp_version "$DEV_HOME/releases")"
export FACTORY_VERSION
PORT=7421
APP_ID="dev.factory.app.dev"
APP="$ROOT/app/build/Factory Dev.app"

backend() {
  cargo build --manifest-path "$ROOT/Cargo.toml" -p factoryd -p claktory-apps -p claktory-speech
  mkdir -p "$DEV_HOME/bin"
  local pidfile="$DEV_HOME/factoryd.pid"
  if [[ -f "$pidfile" ]]; then
    local pid
    pid="$(cat "$pidfile")"
    # Only stop the pid if it's still a dev backend.
    if ps -p "$pid" -o command= 2>/dev/null | grep -q -- 'factoryd --dev'; then
      kill "$pid"
      while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done
    fi
    rm -f "$pidfile"
  fi
  # The supervisor runs ~/.factory-dev/bin/factoryd, which updates replace.
  cp "$ROOT/target/debug/factoryd" "$DEV_HOME/bin/factoryd.new"
  mv -f "$DEV_HOME/bin/factoryd.new" "$DEV_HOME/bin/factoryd"
  # The host that runs apps, which factoryd finds beside itself.
  cp "$ROOT/target/debug/claktory-apps" "$DEV_HOME/bin/claktory-apps.new"
  mv -f "$DEV_HOME/bin/claktory-apps.new" "$DEV_HOME/bin/claktory-apps"
  # Speech recognition for voice messages, which factoryd also finds beside itself.
  cp "$ROOT/target/debug/claktory-speech" "$DEV_HOME/bin/claktory-speech.new"
  mv -f "$DEV_HOME/bin/claktory-speech.new" "$DEV_HOME/bin/claktory-speech"
  rm -f "$DEV_HOME/bin/trial.json"
  # Own session: the backend outlives this script and whatever ran it.
  perl -MPOSIX -e 'POSIX::setsid(); exec @ARGV' -- "$ROOT/target/debug/factoryd" --dev supervise \
    </dev/null >>"$DEV_HOME/factoryd.log" 2>&1 &
  echo $! >"$pidfile"
  for _ in $(seq 100); do
    if nc -z 127.0.0.1 "$PORT" 2>/dev/null; then
      echo "dev backend running on port $PORT (log: $DEV_HOME/factoryd.log)"
      return
    fi
    sleep 0.1
  done
  echo "dev backend didn't start; see $DEV_HOME/factoryd.log" >&2
  exit 1
}

app() {
  (cd "$ROOT/app" && flutter build macos --debug --dart-define=FACTORY_PORT="$PORT" --dart-define=FACTORY_DEV=true --dart-define=FACTORY_VERSION="$FACTORY_VERSION")
  if pgrep -f "Factory Dev.app/Contents/MacOS/" >/dev/null; then
    # Flutter answers the first quit request with "cancelled" and then quits by itself.
    osascript -e "tell application id \"$APP_ID\" to quit" >/dev/null 2>&1 || true
    for _ in $(seq 50); do
      pgrep -f "Factory Dev.app/Contents/MacOS/" >/dev/null || break
      sleep 0.1
    done
    pkill -f "Factory Dev.app/Contents/MacOS/" || true
  fi
  take_build "$ROOT/app/build/macos/Build/Products/Debug/Factory.app" "$APP"
  make_dev_app "$APP" DebugProfile.entitlements
  # -g: open in the background, never take focus from whoever is working.
  open -g "$APP"
  echo "Claktory Dev launched: $APP"
}

case "${1:-all}" in
  backend) backend ;;
  app) app ;;
  all) backend && app ;;
  *) echo "usage: scripts/dev.sh [backend|app]" >&2; exit 2 ;;
esac
