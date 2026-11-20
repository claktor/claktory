#!/usr/bin/env bash
# Installs, updates and looks after Claktory's account service on a Linux
# host you reach with ssh (any distribution with systemd; root, or a user
# with sudo). It builds the service for the host's architecture here
# (scripts/build-account-linux.sh, so podman or docker), copies it over with
# the files in crates/claktory-account/deploy, and runs their install.sh:
# the service under systemd, Caddy in front with a Let's Encrypt
# certificate, daily database backups and DB-IP's GeoIP file. The steps for
# a new host are in crates/claktory-account/DEPLOY.md.
#
#   scripts/deploy-account.sh [options] HOST [COMMAND]
#
# HOST is anything ssh takes (root@203.0.113.7, or a Host in ~/.ssh/config).
# COMMAND:
#   install       install, or update to this checkout's service (the default)
#   resend-key    asks for the Resend API key and puts it in the service's secrets
#   secrets       copies the secrets file here, for you to keep offline
#   backup        copies the newest database backup here
#   status        the service's state and the last lines of its log
#
# Options (for install):
#   --domain NAME          the service's address (account.claktory.com)
#   --admin-email ADDRESS  who runs it: usage counts at /account/admin, and the
#                          email-quota alert; repeat for more
#   --redirect-from NAME   also redirect NAME (all of it) to the service, for
#                          claktory.com/account while claktory.com has no site
#   --mail-domain NAME     emails come from signin@ and notices@ it (auth.claktory.com)
#   --email-daily-cap N    emails a day at most (90; Resend's free plan allows 100)
#   --tls-internal         Caddy's own certificate authority, for trying it out
#                          where DNS doesn't point at the host
#   --no-geoip             leave out the GeoIP file
# And for every command:
#   --ssh-config FILE      the ssh configuration to use
#   --to DIR               where secrets and backup put files
#                          (~/Claktory account/HOST)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DOMAIN=account.claktory.com
INSTALL_ARGS=()
SSH_OPTS=()
TO=""
HOST="" COMMAND=install
while [[ $# -gt 0 ]]; do
  case "$1" in
    --domain) DOMAIN="$2"; shift 2 ;;
    --admin-email | --redirect-from | --mail-domain | --email-daily-cap) INSTALL_ARGS+=("$1" "$2"); shift 2 ;;
    --tls-internal | --no-geoip) INSTALL_ARGS+=("$1"); shift ;;
    --ssh-config) SSH_OPTS+=(-F "$2"); shift 2 ;;
    --to) TO="$2"; shift 2 ;;
    -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    -*) echo "deploy-account: unknown option $1" >&2; exit 2 ;;
    *)
      if [[ -z "$HOST" ]]; then HOST="$1"; else COMMAND="$1"; fi
      shift ;;
  esac
done
[[ -n "$HOST" ]] || { echo "usage: scripts/deploy-account.sh [options] HOST [install|resend-key|secrets|backup|status]" >&2; exit 2; }
TO="${TO:-$HOME/Claktory account/$HOST}"

# Runs a shell command on the host, as root.
remote() {
  ssh "${SSH_OPTS[@]}" -o BatchMode=yes "$HOST" "if [ \"\$(id -u)\" = 0 ]; then sh -c '$1'; else sudo -n sh -c '$1'; fi"
}

case "$COMMAND" in
  install)
    ARCH="$(ssh "${SSH_OPTS[@]}" -o BatchMode=yes "$HOST" uname -m)"
    case "$ARCH" in
      x86_64 | amd64) ARCH=x86_64 ;;
      aarch64 | arm64) ARCH=aarch64 ;;
      *) echo "deploy-account: no build for $ARCH" >&2; exit 1 ;;
    esac
    STAGE="$ROOT/target/account-deploy/$ARCH"
    rm -rf "$STAGE"
    mkdir -p "$STAGE"
    "$ROOT/scripts/build-account-linux.sh" "$ARCH" "$STAGE/claktory-account"
    cp "$ROOT"/crates/claktory-account/deploy/* "$STAGE/"
    echo "Installing on $HOST ($ARCH)…"
    ARGS="--domain $DOMAIN"
    for arg in "${INSTALL_ARGS[@]}"; do ARGS="$ARGS $(printf %q "$arg")"; done
    OUTPUT="$(mktemp)"
    COPYFILE_DISABLE=1 tar --no-xattrs -C "$STAGE" -cf - . | ssh "${SSH_OPTS[@]}" -o BatchMode=yes "$HOST" \
      "set -e; d=\$(mktemp -d); tar -xf - -C \$d; if [ \"\$(id -u)\" = 0 ]; then sh \$d/install.sh $ARGS; else sudo -n sh \$d/install.sh $ARGS; fi; rc=\$?; rm -rf \$d; exit \$rc" \
      | tee "$OUTPUT"
    if grep -q "^NEW SECRETS" "$OUTPUT"; then
      mkdir -p "$TO"
      chmod 700 "$TO"
      remote "cat /var/lib/claktory-account/secrets.json" > "$TO/secrets.json"
      chmod 600 "$TO/secrets.json"
      echo
      echo "The service's new secrets are in $TO/secrets.json."
      echo "Keep a copy offline (a password manager, and on paper or a USB stick somewhere else), then delete this one:"
      echo "without k_share and signing_key, no Claktory can unlock its secrets after a restart."
    fi
    rm -f "$OUTPUT"
    ;;
  resend-key)
    read -rsp "Resend API key (starts with re_): " KEY
    echo
    printf '%s\n' "$KEY" | ssh "${SSH_OPTS[@]}" -o BatchMode=yes "$HOST" \
      "if [ \"\$(id -u)\" = 0 ]; then s=''; else s='sudo -n'; fi; \$s runuser -u claktory-account -- /usr/local/bin/claktory-account set-resend-key /var/lib/claktory-account/secrets.json && \$s systemctl restart claktory-account"
    echo "Emails now go through Resend."
    ;;
  secrets)
    mkdir -p "$TO"
    chmod 700 "$TO"
    remote "cat /var/lib/claktory-account/secrets.json" > "$TO/secrets.json"
    chmod 600 "$TO/secrets.json"
    echo "Copied the secrets to $TO/secrets.json. Keep them offline, then delete this copy."
    ;;
  backup)
    NEWEST="$(remote "ls -1 /var/lib/claktory-account/backups | sort | tail -n 1")"
    [[ -n "$NEWEST" ]] || { echo "deploy-account: no backup yet (the first runs within a day of installing)" >&2; exit 1; }
    mkdir -p "$TO"
    chmod 700 "$TO"
    remote "cat /var/lib/claktory-account/backups/$NEWEST" > "$TO/$NEWEST"
    chmod 600 "$TO/$NEWEST"
    echo "Copied $NEWEST to $TO."
    ;;
  status)
    remote "systemctl --no-pager status claktory-account caddy | sed -n \"1,4p;/caddy.service/,+3p\"; echo; journalctl -u claktory-account -n 15 --no-pager"
    ;;
  *)
    echo "deploy-account: unknown command $COMMAND" >&2
    exit 2
    ;;
esac
