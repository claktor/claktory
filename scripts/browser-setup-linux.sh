#!/usr/bin/env bash
# Claktory's browser on Linux (#435): the one administrator step, as root.
# What it installs, path by path, is scripts/browser-setup-linux.md (contract
# C4); Claktory's setup check looks for exactly those. Running it again
# changes nothing that's already right.
#
#   sudo bash browser-setup-linux.sh [--user NAME]
#
# NAME is the user Claktory runs as (default: whoever ran sudo). Debian and
# Ubuntu only (apt); NixOS takes the lines in browser-setup-linux.md instead.
set -euo pipefail

say() { echo "$*" >&2; }
die() { say "browser-setup-linux.sh: $*"; exit 1; }

person="${SUDO_USER:-}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --user) person="${2:-}"; shift 2 ;;
    *) die "doesn't know $1 (usage: sudo bash browser-setup-linux.sh [--user NAME])" ;;
  esac
done
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
# Root never follows a link someone else could have put there (Fugo): the
# runner's folder is the person's, the profiles the browser's user's.
no_link() { local f; for f; do [[ ! -L "$f" ]] || die "$f is a symbolic link; remove it and run this again"; done; }

[[ "$(id -u)" == 0 ]] || die "run it as root: sudo bash $0"
[[ -n "$person" ]] || die "say who runs Claktory: --user NAME"
[[ "$person" =~ ^[a-z_][a-z0-9_.-]{0,31}$ ]] || die "$person isn't a user name this can put in a sudoers line"
[[ "$person" != root && "$person" != claktory-browser ]] || die "Claktory's user can't be $person"
id "$person" >/dev/null 2>&1 || die "there's no user $person"
[[ -f /etc/NIXOS ]] && die "this is NixOS: add the lines in browser-setup-linux.md to configuration.nix instead"
command -v apt-get >/dev/null || die "this needs apt (Debian or Ubuntu); on other systems, the setup card says what to install"
[[ -f "$here/claktory-browser.nft" && -f "$here/claktory-browser-nft.sh" ]] || die "claktory-browser.nft and claktory-browser-nft.sh must be beside this script"

case "$(dpkg --print-architecture)" in
  amd64 | arm64) arch="$(dpkg --print-architecture)" ;;
  *) die "Chrome for Linux comes for amd64 and arm64 only, not $(dpkg --print-architecture)" ;;
esac

# Packages: Chrome from Google's own repository (its key pinned in a keyring
# only that source trusts), Xvfb, xauth, nftables and fonts.
export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -yq ca-certificates curl gnupg
install -d -m 0755 /etc/apt/keyrings
# Google's signing key, pinned by its fingerprint (Fugo), as
# https://www.google.com/linuxrepositories/ lists it; only that key goes in the
# keyring, not the obsolete one served beside it.
google_fpr=EB4C1BFD4F042F6DDDCCEC917721F63BD38B4796
gnupg="$(mktemp -d)"
trap 'rm -rf "$tmp" "$gnupg"' EXIT
if [[ -s /etc/apt/keyrings/google-chrome.gpg ]]; then
  # Its primary keys: exactly Google's, nothing beside it.
  have="$(gpg --homedir "$gnupg" --show-keys --with-colons /etc/apt/keyrings/google-chrome.gpg 2>/dev/null |
    awk -F: '$1=="pub"||$1=="sub"{t=$1} $1=="fpr"&&t=="pub"{print $10}')"
  [[ "$have" == "$google_fpr" ]] || die "/etc/apt/keyrings/google-chrome.gpg isn't Google's key $google_fpr; remove it and run this again"
else
  curl -fsSL https://dl.google.com/linux/linux_signing_key.pub >"$tmp"
  gpg --homedir "$gnupg" --quiet --import "$tmp" 2>/dev/null
  gpg --homedir "$gnupg" --list-keys --with-colons "$google_fpr" >/dev/null 2>&1 ||
    die "Google's key file doesn't hold the key $google_fpr; check https://www.google.com/linuxrepositories/"
  gpg --homedir "$gnupg" --export "$google_fpr" >"$tmp"
  install -o root -g root -m 0644 "$tmp" /etc/apt/keyrings/google-chrome.gpg
fi
# Our source only brings Chrome in: its package then replaces it with its own
# (google-chrome.sources, signed by the key it ships), which keeps Chrome
# updated. Once that's there, never add ours beside it.
if [[ ! -f /etc/apt/sources.list.d/google-chrome.sources ]]; then
  echo "deb [arch=$arch signed-by=/etc/apt/keyrings/google-chrome.gpg] https://dl.google.com/linux/chrome/deb/ stable main" \
    >/etc/apt/sources.list.d/google-chrome.list
fi
apt-get update -q
apt-get install -yq google-chrome-stable xvfb xauth nftables tzdata \
  fonts-liberation fonts-liberation2 fonts-dejavu-core fonts-noto-core fonts-noto-color-emoji fonts-noto-cjk

# The browser's own user: no login, its home and profiles 0700.
id claktory-browser >/dev/null 2>&1 ||
  useradd --system --home-dir /var/lib/claktory-browser --create-home --shell /usr/sbin/nologin claktory-browser
# Root touches only the home, whose parent is root's; the profiles folder sits
# inside the browser's user's home, so that user makes it (Fugo: no race).
no_link /var/lib/claktory-browser
install -d -o claktory-browser -g claktory-browser -m 0700 /var/lib/claktory-browser
runuser -u claktory-browser -- install -d -m 0700 /var/lib/claktory-browser/profiles

# The runner's folder, the person's: factoryd copies itself there before each
# start. Not writable by claktory-browser (Fugo).
no_link /opt/claktory-browser /opt/claktory-browser/runner
install -d -o "$person" -g "$(id -gn "$person")" -m 0755 /opt/claktory-browser
if [[ -e /opt/claktory-browser/runner ]]; then
  [[ -f /opt/claktory-browser/runner ]] || die "/opt/claktory-browser/runner isn't a regular file; remove it and run this again"
  chown -h "$person:$(id -gn "$person")" /opt/claktory-browser/runner
fi

# Ubuntu 23.10 and later refuse user namespaces to unprivileged programs
# unless AppArmor allows them. The runner needs one for its own /proc and PID 1
# (Ghiaccio's finding), and refuses to start without it, so allow exactly that
# path, the way Ubuntu's own chrome profile does.
if [[ -e /proc/sys/kernel/apparmor_restrict_unprivileged_userns && -f /etc/apparmor.d/abi/4.0 ]]; then
  no_link /etc/apparmor.d/claktory-browser-runner
  cat >"$tmp" <<'EOF'
abi <abi/4.0>,
include <tunables/global>

# Claktory's browser (#435): its runner may make user namespaces, nothing else.
profile claktory-browser-runner /opt/claktory-browser/runner flags=(unconfined) {
  userns,

  include if exists <local/claktory-browser-runner>
}
EOF
  if ! cmp -s "$tmp" /etc/apparmor.d/claktory-browser-runner; then
    install -o root -g root -m 0644 "$tmp" /etc/apparmor.d/claktory-browser-runner
  fi
  if [[ -d /sys/kernel/security/apparmor ]] && command -v apparmor_parser >/dev/null; then
    apparmor_parser -r /etc/apparmor.d/claktory-browser-runner || die "AppArmor refuses /etc/apparmor.d/claktory-browser-runner"
    if command -v aa-status >/dev/null; then
      aa-status 2>/dev/null | grep -qw claktory-browser-runner || die "AppArmor didn't load claktory-browser-runner (aa-status)"
    else
      grep -q '^claktory-browser-runner ' /sys/kernel/security/apparmor/profiles || die "AppArmor didn't load claktory-browser-runner"
    fi
  else
    say "note: AppArmor isn't running here, so /etc/apparmor.d/claktory-browser-runner isn't loaded"
  fi
fi

# Sudo for exactly the runner, as the browser's user. sudo closes inherited
# descriptors above 2 (closefrom) and nothing here overrides that.
line="$person ALL=(claktory-browser) NOPASSWD: /opt/claktory-browser/runner browser-runner *"
echo "$line" >"$tmp"
visudo -cqf "$tmp" || die "sudo refuses the line: $line"
if ! cmp -s "$tmp" /etc/sudoers.d/claktory-browser; then
  install -o root -g root -m 0440 "$tmp" /etc/sudoers.d/claktory-browser
fi

# X's socket folder, root's and 1777, as systemd's own x11.conf makes it: each
# runner mounts its own tmpfs over it and refuses to start unless it's there
# as a real folder of root's (Ghiaccio, Fugo 38527). A minimal server may lack it.
printf 'D! /tmp/.X11-unix 1777 root root -\n' >"$tmp"
if ! cmp -s "$tmp" /etc/tmpfiles.d/claktory-browser.conf; then
  install -o root -g root -m 0644 "$tmp" /etc/tmpfiles.d/claktory-browser.conf
fi
# Never adopt one someone else made: its contents (an X socket) would stay.
[[ ! -L /tmp/.X11-unix ]] || die "/tmp/.X11-unix is a symbolic link; remove it and run this again"
if [[ -e /tmp/.X11-unix ]]; then
  [[ -d /tmp/.X11-unix && ! -L /tmp/.X11-unix && "$(stat -c %u /tmp/.X11-unix)" == 0 ]] ||
    die "/tmp/.X11-unix belongs to $(stat -c %U /tmp/.X11-unix 2>/dev/null || echo someone else): remove it (it's made again at boot) and run this again"
fi
if [[ -d /run/systemd/system ]]; then
  systemd-tmpfiles --create /etc/tmpfiles.d/claktory-browser.conf
else
  [[ ! -L /tmp/.X11-unix ]] || die "/tmp/.X11-unix is a symbolic link; remove it and run this again"
  install -d -o root -g root -m 1777 /tmp/.X11-unix
fi
[[ -d /tmp/.X11-unix && ! -L /tmp/.X11-unix && "$(stat -c %u /tmp/.X11-unix)" == 0 ]] || die "/tmp/.X11-unix isn't a folder of root's"

# Chrome's policy lives only in the runner's own mount namespace (Ghiaccio
# 49028, Fugo 49032): the runner mounts a private tmpfs over Chrome's managed
# policy folder and writes it there, so the person's own Chrome on this
# machine never sees it. Here: the empty mount points, real folders of
# root's, and our own policy file from before taken out. Any other file there
# (an employer's policy for the person's Chrome, say) is left as it is.
our_policy='"LocalNetworkAccessBlockedForUrls"'
for dir in /etc/opt/chrome/policies/managed /etc/chromium/policies/managed; do
  # Chromium's folder only where it's there already or Chromium is installed.
  if [[ "$dir" == /etc/chromium/* && ! -e "$dir" ]] && ! command -v chromium >/dev/null && ! command -v chromium-browser >/dev/null; then continue; fi
  for part in "${dir%/policies/managed}" "${dir%/managed}" "$dir"; do
    [[ ! -L "$part" ]] || die "$part is a symbolic link; remove it and run this again"
  done
  install -d -o root -g root -m 0755 "$dir"
  [[ "$(stat -c %u "$dir")" == 0 ]] || die "$dir isn't a folder of root's"
  if [[ -f "$dir/claktory.json" ]]; then
    grep -q "$our_policy" "$dir/claktory.json" || die "$dir/claktory.json isn't Claktory's policy; it's left as it is, and the browser can't be set up while it's there"
    rm -f "$dir/claktory.json"
  fi
done

# The firewall: the browser's user reaches neither this machine nor the local
# network. Its own unit loads only its own table, now and at every boot, so the
# machine's firewall (nftables.service, whose stock file flushes everything, ufw,
# Docker) is never touched; when nftables.service restarts, this one follows.
nft="$(command -v nft)"
no_link /etc/claktory-browser.nft /usr/local/sbin/claktory-browser-nft
if ! cmp -s "$here/claktory-browser.nft" /etc/claktory-browser.nft; then
  install -o root -g root -m 0644 "$here/claktory-browser.nft" /etc/claktory-browser.nft
fi
"$nft" -c -f /etc/claktory-browser.nft || die "nft refuses /etc/claktory-browser.nft"
# An earlier version of this script added an include to /etc/nftables.conf.
[[ ! -f /etc/nftables.conf ]] || sed -i '\|^include "/etc/claktory-browser.nft"$|d' /etc/nftables.conf
if ! cmp -s "$here/claktory-browser-nft.sh" /usr/local/sbin/claktory-browser-nft; then
  install -o root -g root -m 0755 "$here/claktory-browser-nft.sh" /usr/local/sbin/claktory-browser-nft
fi
cat >"$tmp" <<EOF
[Unit]
Description=Claktory's browser: firewall for its user
DefaultDependencies=no
Wants=network-pre.target
Before=network-pre.target shutdown.target
After=nftables.service
PartOf=nftables.service
Conflicts=shutdown.target

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/sbin/claktory-browser-nft
ExecReload=/usr/local/sbin/claktory-browser-nft

[Install]
WantedBy=sysinit.target
EOF
if ! cmp -s "$tmp" /etc/systemd/system/claktory-browser-nft.service; then
  install -o root -g root -m 0644 "$tmp" /etc/systemd/system/claktory-browser-nft.service
fi
if [[ -d /run/systemd/system ]]; then
  systemctl daemon-reload
  systemctl enable claktory-browser-nft.service >/dev/null 2>&1
  systemctl restart claktory-browser-nft.service
else
  say "note: systemd isn't running here, so the firewall is loaded now but won't come back after a restart"
  /usr/local/sbin/claktory-browser-nft
fi

# What Claktory's setup check will see.
echo "Claktory's browser is set up for $person: $(google-chrome-stable --version 2>/dev/null || echo 'Chrome: not found'), user claktory-browser, the policy and the firewall (nft table inet claktory_browser)."
abi="$(cat /sys/kernel/security/lsm 2>/dev/null | tr ',' '\n' | grep -c '^landlock$' || true)"
[[ "$abi" == 1 ]] || say "note: Landlock isn't among this kernel's security modules (/sys/kernel/security/lsm); Claktory refuses to run its browser without it"
