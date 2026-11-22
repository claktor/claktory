#!/usr/bin/env bash
# Installs one release folder as the real Factory, for the first time or to
# repair it. Both installers end here: scripts/install.sh (from a checkout)
# and the online installer (scripts/install-online.sh), which runs the copy of
# this script that comes in each published release, so it needs nothing else
# from the repo.
#   ~/.factory/bin/factoryd     the backend, which updates replace
#   ~/.factory/bin/claktory-apps  the apps host beside it, which runs apps
#   ~/.factory/bin/claktory-speech  the speech host, which writes what voice
#                               messages say
#   ~/.factory/bin/Claktory Agents.app  the supervisor, which keeps it
#                               running and undoes updates that don't start; a
#                               LaunchAgent runs it. An app, so it can be added
#                               under Privacy & Security → Developer Tools.
#                               (Releases made before it had one install the
#                               bare ~/.factory/bin/factoryd-supervisor.)
#   ~/Applications/Factory.app  the app
#
#   install-release.sh RELEASE_DIR [--open]
#
# --open opens the app afterwards; otherwise it's reopened only if it was open.
# The backend restarts, which interrupts every agent mid-run.
set -euo pipefail

RELEASE="${1:?usage: install-release.sh RELEASE_DIR [--open] [--hold]}"
OPEN=false
HOLD=false
for flag in "${@:2}"; do
  case "$flag" in
    --open) OPEN=true ;;
    # Installed but not started: the online installer's --import runs
    # `factoryd import` first, then installs again without --hold.
    --hold) HOLD=true ;;
    *) echo "install-release.sh doesn't know $flag" >&2; exit 1 ;;
  esac
done
HOME_DIR="$HOME/.factory"
BIN="$HOME_DIR/bin/factoryd"
APP="$HOME/Applications/Factory.app"
LABEL="dev.factory.factoryd"
AGENT_PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"

if [[ ! -f "$RELEASE/factoryd" || ! -d "$RELEASE/Factory.app" ]]; then
  echo "$RELEASE isn't a Claktory release: it needs factoryd and Factory.app" >&2
  exit 1
fi

# Puts the bundle NEW where OLD is. The old one moves aside first, so a power
# cut leaves one of them in place; a bundle left aside by an earlier cut comes
# back if nothing took its place.
swap_in() {
  local new="$1" old="$2"
  [[ -e "$old" || ! -e "$old.old" ]] || mv "$old.old" "$old"
  rm -rf "$old.old"
  [[ ! -e "$old" ]] || mv "$old" "$old.old"
  mv "$new" "$old"
  rm -rf "$old.old"
}

mkdir -p "$HOME_DIR/bin" "$HOME/Applications" "$HOME/Library/LaunchAgents"
# Only this user reads it: its keys and agents' work go here, even before a
# backend that can't start yet (another user's holds its port) keeps it so.
chmod 700 "$HOME_DIR"
cp "$RELEASE/factoryd" "$BIN.new"
chmod 755 "$BIN.new"
mv -f "$BIN.new" "$BIN"
# The apps host, beside the backend (releases before apps have none).
if [[ -f "$RELEASE/claktory-apps" ]]; then
  cp "$RELEASE/claktory-apps" "$HOME_DIR/bin/claktory-apps.new"
  chmod 755 "$HOME_DIR/bin/claktory-apps.new"
  mv -f "$HOME_DIR/bin/claktory-apps.new" "$HOME_DIR/bin/claktory-apps"
fi
# Claktory's own apps, beside them (apps/<id>.clapp), the folder replaced
# whole so an app a release no longer ships goes too.
rm -rf "$HOME_DIR/bin/apps.new"
if [[ -d "$RELEASE/apps" ]]; then
  cp -R "$RELEASE/apps" "$HOME_DIR/bin/apps.new"
  rm -rf "$HOME_DIR/bin/apps"
  mv "$HOME_DIR/bin/apps.new" "$HOME_DIR/bin/apps"
else
  rm -rf "$HOME_DIR/bin/apps"
fi
# The speech host, beside them (releases before voice have none).
if [[ -f "$RELEASE/claktory-speech" ]]; then
  cp "$RELEASE/claktory-speech" "$HOME_DIR/bin/claktory-speech.new"
  chmod 755 "$HOME_DIR/bin/claktory-speech.new"
  mv -f "$HOME_DIR/bin/claktory-speech.new" "$HOME_DIR/bin/claktory-speech"
fi
SUPERVISOR_APP="$HOME_DIR/bin/Claktory Agents.app"
if [[ -d "$RELEASE/supervisor/Claktory Agents.app" ]]; then
  rm -rf "$SUPERVISOR_APP.new"
  ditto "$RELEASE/supervisor/Claktory Agents.app" "$SUPERVISOR_APP.new"
  swap_in "$SUPERVISOR_APP.new" "$SUPERVISOR_APP"
  SUPERVISOR="$SUPERVISOR_APP/Contents/MacOS/factoryd-supervisor"
else
  cp "$RELEASE/factoryd" "$BIN-supervisor.new"
  chmod 755 "$BIN-supervisor.new"
  mv -f "$BIN-supervisor.new" "$BIN-supervisor"
  SUPERVISOR="$BIN-supervisor"
fi
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
swap_in "$APP.new" "$APP"

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
  <key>ProgramArguments</key><array><string>$SUPERVISOR</string><string>supervise</string></array>
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
# The supervisor that ran before, when it's now the app.
[[ "$SUPERVISOR" == "$BIN-supervisor" ]] || rm -f "$BIN-supervisor"
if $HOLD; then
  echo "Claktory installed but not started (--hold)."
  exit 0
fi
launchctl bootstrap "gui/$(id -u)" "$AGENT_PLIST"
if $OPEN; then
  open "$APP"
elif $was_open; then
  open -g "$APP"
fi
echo "Claktory installed. Backend log: $HOME_DIR/factoryd.log"
