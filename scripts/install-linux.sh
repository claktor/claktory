#!/usr/bin/env bash
# Installs one Linux release folder as the Factory on this machine, for the
# first time or to repair it. The online installer (scripts/install-online.sh)
# runs the copy of this script that comes in each published Linux release, so
# it needs nothing else from the repo.
#   ~/.factory/bin/factoryd             the backend, which updates replace
#   ~/.factory/bin/factoryd-supervisor  keeps it running and undoes updates
#                                       that don't start; a systemd user
#                                       service runs it, from boot on
#   ~/.config/systemd/user/factoryd.service
#
#   install-linux.sh RELEASE_DIR
#
# A Linux Factory has no app of its own: Factory on a Mac connects to it over
# its private tunnel. The first install prints a pairing code for the first
# device. Installing again restarts the
# backend, which interrupts every agent mid-run.
set -euo pipefail

RELEASE="${1:?usage: install-linux.sh RELEASE_DIR}"
HOME_DIR="$HOME/.factory"
BIN="$HOME_DIR/bin/factoryd"
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
UNIT="$UNIT_DIR/factoryd.service"

fail() {
  echo "Claktory can't be installed: $*" >&2
  exit 1
}

[[ "$(uname -s)" == Linux ]] || fail "this installer is for Linux."
[[ -f "$RELEASE/factoryd" ]] || fail "$RELEASE isn't a Claktory release: it needs factoryd."
command -v systemctl >/dev/null || fail "it needs systemd, to keep Claktory running."
# systemctl --user needs the user's session; a login over ssh has one.
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
systemctl --user show-environment >/dev/null 2>&1 ||
  fail "systemd doesn't run a session for $(id -un) (systemctl --user doesn't answer). Log in over ssh or a terminal, not su or sudo, and try again."

first=true
[[ -f "$HOME_DIR/config.json" ]] && first=false

mkdir -p "$HOME_DIR/bin" "$UNIT_DIR"
cp "$RELEASE/factoryd" "$BIN.new"
chmod 755 "$BIN.new"
mv -f "$BIN.new" "$BIN"
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
systemctl --user restart factoryd.service
echo "Claktory installed. Its log: $HOME_DIR/factoryd.log"

if $first; then
  # The backend writes its settings when it first starts.
  for _ in $(seq 100); do
    [[ -f "$HOME_DIR/config.json" ]] && break
    sleep 0.2
  done
  [[ -f "$HOME_DIR/config.json" ]] || fail "the backend didn't start; see $HOME_DIR/factoryd.log"
  echo
  if loginctl show-user "$(id -un)" -p Linger 2>/dev/null | grep -q yes; then
    echo "Claktory is running on this machine and starts with it."
  else
    echo "Claktory is running on this machine while you're logged in (see above to keep it running)."
  fi
  echo
  "$BIN" pair "First device"
  echo
  echo "To connect: on your iPhone, open Claktory and tap Scan code. On your Mac, click Add a Claktory (+) at the far left of the window and paste the code. It works for 10 minutes."
  echo "To connect another device later, run: ~/.factory/bin/factoryd pair \"Device name\""
fi
