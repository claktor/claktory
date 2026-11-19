#!/usr/bin/env bash
# Installs one release folder as the real Factory, for the first time or to
# repair it. Both installers end here: scripts/install.sh (from a checkout)
# and the online installer (scripts/install-online.sh), which runs the copy of
# this script that comes in each published release, so it needs nothing else
# from the repo.
#   ~/.factory/bin/factoryd     the backend, which updates replace
#   ~/.factory/bin/factoryd-supervisor  keeps it running and undoes updates
#                               that don't start; a LaunchAgent runs it
#   ~/Applications/Factory.app  the app
#
#   install-release.sh RELEASE_DIR [--open]
#
# --open opens the app afterwards; otherwise it's reopened only if it was open.
# The backend restarts, which interrupts every agent mid-run.
set -euo pipefail

RELEASE="${1:?usage: install-release.sh RELEASE_DIR [--open]}"
OPEN=false
[[ "${2:-}" == "--open" ]] && OPEN=true
HOME_DIR="$HOME/.factory"
BIN="$HOME_DIR/bin/factoryd"
APP="$HOME/Applications/Factory.app"
LABEL="dev.factory.factoryd"
AGENT_PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"

if [[ ! -f "$RELEASE/factoryd" || ! -d "$RELEASE/Factory.app" ]]; then
  echo "$RELEASE isn't a Factory release: it needs factoryd and Factory.app" >&2
  exit 1
fi

mkdir -p "$HOME_DIR/bin" "$HOME/Applications" "$HOME/Library/LaunchAgents"
cp "$RELEASE/factoryd" "$BIN.new"
chmod 755 "$BIN.new"
mv -f "$BIN.new" "$BIN"
cp "$RELEASE/factoryd" "$BIN-supervisor.new"
chmod 755 "$BIN-supervisor.new"
mv -f "$BIN-supervisor.new" "$BIN-supervisor"
# A fresh install isn't an update on trial.
rm -f "$HOME_DIR/bin/trial.json"

# An open app keeps running the old version; restart it after the swap.
was_open=false
if pgrep -f "$APP/Contents/MacOS/" >/dev/null; then
  was_open=true
  # Flutter answers the first quit request with "cancelled" and then quits by itself.
  osascript -e 'tell application id "dev.factory.app" to quit' >/dev/null 2>&1 || true
  for _ in $(seq 50); do
    pgrep -f "$APP/Contents/MacOS/" >/dev/null || break
    sleep 0.1
  done
  pkill -f "$APP/Contents/MacOS/" || true
fi
rm -rf "$APP.new"
ditto "$RELEASE/Factory.app" "$APP.new"
rm -rf "$APP"
mv "$APP.new" "$APP"

# Agents run shell commands through factoryd, so it needs this shell's PATH
# (cargo, flutter, git, ...), not launchd's minimal one. Interactive: without a
# ProcessType launchd throttles the job's processor and disk use and stretches
# its timers (a 10 ms sleep took 73 ms), and every agent's build, test and
# server inherits that; the person's work should run as it does in a terminal.
cat >"$AGENT_PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key><array><string>$BIN-supervisor</string><string>supervise</string></array>
  <key>EnvironmentVariables</key><dict><key>PATH</key><string>$PATH</string></dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Interactive</string>
  <key>StandardOutPath</key><string>$HOME_DIR/factoryd.log</string>
  <key>StandardErrorPath</key><string>$HOME_DIR/factoryd.log</string>
</dict>
</plist>
PLIST

launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
while launchctl print "gui/$(id -u)/$LABEL" >/dev/null 2>&1; do sleep 0.1; done
launchctl bootstrap "gui/$(id -u)" "$AGENT_PLIST"
if $OPEN; then
  open "$APP"
elif $was_open; then
  open -g "$APP"
fi
echo "Factory installed. Backend log: $HOME_DIR/factoryd.log"
