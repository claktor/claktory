#!/usr/bin/env bash
# Installs one Linux release folder as the Factory on this machine, for the
# first time or to repair it. The online installer (scripts/install-online.sh)
# runs the copy of this script that comes in each published Linux release, so
# it needs nothing else from the repo.
#   ~/.factory/bin/factoryd             the backend, which updates replace
#   ~/.factory/bin/claktory-apps        the apps host beside it, which runs apps
#   ~/.factory/bin/factoryd-supervisor  keeps it running and undoes updates
#                                       that don't start; a systemd user
#                                       service runs it, from boot on
#   ~/.config/systemd/user/factoryd.service
#
#   ~/.local/bin/factoryd               a link to the backend, so factoryd is
#                                       on PATH (/usr/local/bin/factoryd for root)
#
#   install-linux.sh RELEASE_DIR [--no-pair]
#
# A Linux Factory has no app of its own: Factory on a Mac connects to it over
# its private tunnel. The first install prints a pairing code for the first
# device and waits for it (--no-pair leaves that out: the online installer's
# --yes and --json). Installing again restarts the backend, which interrupts
# every agent mid-run.
#
# Exit codes, as factoryd's commands use them: 0 installed and running, 3
# refused (not this machine's kind, or no systemd session), 4 installed but
# not running (the reason on stderr), 1 anything else.
set -euo pipefail

RELEASE="${1:?usage: install-linux.sh RELEASE_DIR [--no-pair] [--hold]}"
PAIR=true
HOLD=false
for flag in "${@:2}"; do
  case "$flag" in
    --no-pair) PAIR=false ;;
    # Installed and enabled but not started: the online installer's --import
    # runs `factoryd import` first, then installs again without --hold.
    --hold) HOLD=true ;;
    *) echo "install-linux.sh doesn't know $flag" >&2; exit 1 ;;
  esac
done
HOME_DIR="$HOME/.factory"
BIN="$HOME_DIR/bin/factoryd"
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
UNIT="$UNIT_DIR/factoryd.service"
PORT="${FACTORY_PORT:-7420}"

fail() {
  echo "Claktory can't be installed: $*" >&2
  exit 1
}
refuse() {
  echo "Claktory can't be installed: $*" >&2
  exit 3
}

[[ "$(uname -s)" == Linux ]] || refuse "this installer is for Linux."
[[ -f "$RELEASE/factoryd" ]] || fail "$RELEASE isn't a Claktory release: it needs factoryd."
command -v systemctl >/dev/null || refuse "it needs systemd, to keep Claktory running."
# systemctl --user needs the user's session; a login over ssh has one.
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
systemctl --user show-environment >/dev/null 2>&1 ||
  refuse "systemd doesn't run a session for $(id -un) (systemctl --user doesn't answer). Log in over ssh or a terminal, not su or sudo, and try again."

first=true
[[ -f "$HOME_DIR/config.json" ]] && first=false

mkdir -p "$HOME_DIR/bin" "$UNIT_DIR"
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
# Agents' tools (ripgrep), in <home>/tools, which factoryd puts first on
# agents' PATH (releases before it have none). Its licenses go with it.
if [[ -f "$RELEASE/tools/rg" ]]; then
  mkdir -p "$HOME_DIR/tools"
  cp "$RELEASE/tools/rg" "$HOME_DIR/tools/rg.new"
  chmod 755 "$HOME_DIR/tools/rg.new"
  mv -f "$HOME_DIR/tools/rg.new" "$HOME_DIR/tools/rg"
  if [[ -d "$RELEASE/tools/LICENSES" ]]; then rm -rf "$HOME_DIR/tools/LICENSES"; cp -R "$RELEASE/tools/LICENSES" "$HOME_DIR/tools/LICENSES"; fi
fi
cp "$RELEASE/factoryd" "$BIN-supervisor.new"
chmod 755 "$BIN-supervisor.new"
mv -f "$BIN-supervisor.new" "$BIN-supervisor"
# A fresh install isn't an update on trial.
rm -f "$HOME_DIR/bin/trial.json"

# Agents run shell commands through factoryd, so it needs this shell's PATH,
# not systemd's minimal one; on NixOS, without the /nix/store folders a
# nix-shell adds, which go away (its profiles' bin folders stay).
UNIT_PATH="$(tr ':' '\n' <<<"$PATH" | grep -v '^/nix/store/' | paste -sd: -)"
nixos=false
[[ -f /etc/NIXOS ]] && nixos=true

# Agents need git; a minimal system (NixOS's, say) may not have it.
command -v git >/dev/null ||
  echo "Your agents will need git, which this machine doesn't have yet. Install it (on NixOS, add git to environment.systemPackages or your user's packages)." >&2

# Agents run shell commands through factoryd (above). KillMode=process: stopping the service stops
# only the supervisor, which stops the backend itself; agents' background
# tasks keep running, as they do through updates.
cat >"$UNIT" <<UNIT
[Unit]
Description=Claktory
After=network.target

[Service]
ExecStart=$BIN-supervisor supervise
Environment=PATH=$UNIT_PATH
WorkingDirectory=$HOME
Restart=always
RestartSec=2
KillMode=process
StandardOutput=append:$HOME_DIR/factoryd.log
StandardError=append:$HOME_DIR/factoryd.log

[Install]
WantedBy=default.target
UNIT

# Without lingering, systemd stops the user's services when they log out.
# Turning it on for yourself takes polkit's leave, which a login over ssh
# may not have; then it takes root. NixOS leaves lingering to loginctl
# unless the configuration sets users.users.<name>.linger, which then wins.
if ! loginctl show-user "$(id -un)" -p Linger 2>/dev/null | grep -q yes; then
  if ! loginctl enable-linger "$(id -un)" 2>/dev/null; then
    if $nixos; then
      echo "Claktory stops when you log out until lingering is on for you: add  users.users.$(id -un).linger = true;  to your NixOS configuration and run nixos-rebuild switch (or, until then: sudo loginctl enable-linger $(id -un))." >&2
    else
      echo "Claktory stops when you log out until lingering is on for you: sudo loginctl enable-linger $(id -un)" >&2
    fi
  fi
fi
systemctl --user daemon-reload
systemctl --user enable factoryd.service >/dev/null 2>&1
if $HOLD; then
  # Started later: a backend would refuse to run mid-import anyway.
  systemctl --user stop factoryd.service 2>/dev/null || true
  echo "Claktory installed but not started (--hold)."
  exit 0
fi
log_start=0
[[ -f "$HOME_DIR/factoryd.log" ]] && log_start="$(stat -c %s "$HOME_DIR/factoryd.log")"
systemctl --user restart factoryd.service

# Running once it answers its own commands (it writes its settings first, on
# a first start). If it doesn't, the reason it logged since the restart, or
# whoever holds its port. The online installer reads its status and the command's path from
# CLAKTORY_INSTALL_OUT, for its --json answer.
out="${CLAKTORY_INSTALL_OUT:-}"
if [[ -z "$out" ]]; then
  out="$(mktemp -d)"
  trap 'rm -rf "$out"' EXIT
fi
# Whether whoever listens on the port is another user's: ss names the process
# only for one's own (or for root).
others_port() {
  command -v ss >/dev/null || return 1
  local listener
  listener="$(ss -ltnpH "sport = :$PORT" 2>/dev/null)"
  [[ -n "$listener" ]] || return 1
  [[ "$(id -u)" == 0 ]] && return 1
  [[ "$listener" != *pid=* ]] || return 1
  # factoryd isn't dumpable (it keeps the vault from other processes), so
  # ss can't name it even to its own user: it's ours if we run a factoryd.
  ! pgrep -u "$(id -u)" -x factoryd >/dev/null 2>&1
}
running=false
for _ in $(seq 150); do
  # Not another user's backend answering in its place.
  if "$BIN" status --json >"$out/status.json" 2>/dev/null && ! others_port; then
    running=true
    break
  fi
  # It said why it stopped, and someone else has its port: no use waiting.
  if others_port && tail -c +"$((log_start + 1))" "$HOME_DIR/factoryd.log" 2>/dev/null | grep -q 'factoryd stopped error='; then
    break
  fi
  sleep 0.2
done
if ! $running; then
  reason="$(tail -c +"$((log_start + 1))" "$HOME_DIR/factoryd.log" 2>/dev/null | sed -n 's/.*factoryd stopped error=//p' | tail -n 1)"
  if [[ -z "$reason" ]] && others_port; then
    reason="Another Claktory on this computer already uses port $PORT (another user's, say), so this one can't start. Stop that one, or start this one on another port with FACTORY_PORT."
  fi
  [[ -n "$reason" ]] || reason="it didn't start. Its log: $HOME_DIR/factoryd.log"
  echo "Claktory is installed but isn't running: $reason" >&2
  exit 4
fi
version="$(sed -n 's/.*"version":"\([^"]*\)".*/\1/p' "$out/status.json")"

# factoryd on PATH: a link in /usr/local/bin for root, else ~/.local/bin,
# which most distributions put on PATH at login once it exists. Never over
# another program of that name.
cli="$BIN"
link_dir="$HOME/.local/bin"
[[ "$(id -u)" == 0 ]] && link_dir=/usr/local/bin
if mkdir -p "$link_dir" 2>/dev/null && { [[ -L "$link_dir/factoryd" ]] || [[ ! -e "$link_dir/factoryd" ]]; }; then
  ln -sfn "$BIN" "$link_dir/factoryd"
  cli="$link_dir/factoryd"
fi
case ":$PATH:" in
  *":$(dirname "$cli"):"*) on_path=true ;;
  *) on_path=false ;;
esac
echo "$cli" >"$out/cli"

if loginctl show-user "$(id -un)" -p Linger 2>/dev/null | grep -q yes; then
  echo "Claktory ${version:-} is running on this machine and starts with it. Its log: $HOME_DIR/factoryd.log"
else
  echo "Claktory ${version:-} is running on this machine while you're logged in (see above to keep it running). Its log: $HOME_DIR/factoryd.log"
fi
if $on_path; then
  echo "Its command, factoryd, is in $(dirname "$cli")."
  run="factoryd"
else
  if grep -qs '\.local/bin' "$HOME/.profile" "$HOME/.bash_profile" "$HOME/.bash_login"; then
    echo "Its command, factoryd, is in $(dirname "$cli"), which your next login puts on your PATH. Until then: export PATH=\"$(dirname "$cli"):\$PATH\""
  else
    echo "Its command, factoryd, is in $(dirname "$cli"), which isn't on your PATH: add  export PATH=\"$(dirname "$cli"):\$PATH\"  to your shell's profile."
  fi
  run="$cli"
fi

if $first && $PAIR; then
  echo
  # In a terminal, pair waits there until the device is allowed.
  # Cancelling it (Ctrl-C) leaves Claktory installed all the same.
  "$BIN" pair || true
  echo
  echo "To connect another device later, run: $run pair"
elif $first; then
  echo "To connect a device, run: $run pair"
fi
