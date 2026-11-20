#!/bin/sh
# Installs or updates Claktory's account service on this Linux host, with
# Caddy in front for TLS. scripts/deploy-account.sh copies this folder here
# and runs it as root; it can be run again any time (an update is the same
# run with a new binary). If the new service doesn't answer, the previous
# binary is put back.
#
#   sh install.sh --domain account.claktory.com [options]
#
#   --domain NAME          the service's address (Caddy gets its certificate)
#   --port N               where the service listens on 127.0.0.1 (8417)
#   --admin-email ADDRESS  who runs it (repeat for more): usage counts at
#                          /account/admin, and the email-quota alert
#   --mail-domain NAME     emails come from signin@ and notices@ it (auth.claktory.com)
#   --email-daily-cap N    emails a day at most (90)
#   --redirect-from NAME   also serve NAME, redirecting it all to --domain
#                          (for claktory.com/account while claktory.com has no site)
#   --tls-internal         Caddy's own certificate authority instead of Let's
#                          Encrypt: for trying it out where DNS doesn't point here
#   --no-geoip             don't fetch DB-IP's free GeoIP city file
set -eu

DOMAIN="" PORT=8417 ADMINS="" MAIL_DOMAIN="" CAP="" REDIRECT="" TLS="" GEOIP=1
while [ $# -gt 0 ]; do
  case "$1" in
    --domain) DOMAIN="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    --admin-email) ADMINS="$ADMINS --admin-email $2"; shift 2 ;;
    --mail-domain) MAIL_DOMAIN="$2"; shift 2 ;;
    --email-daily-cap) CAP="$2"; shift 2 ;;
    --redirect-from) REDIRECT="$2"; shift 2 ;;
    --tls-internal) TLS=internal; shift ;;
    --no-geoip) GEOIP=0; shift ;;
    *) echo "install.sh: unknown option $1" >&2; exit 2 ;;
  esac
done
[ -n "$DOMAIN" ] || { echo "install.sh: --domain is needed" >&2; exit 2; }
[ "$(id -u)" = 0 ] || { echo "install.sh: run it as root" >&2; exit 1; }
command -v systemctl >/dev/null || { echo "install.sh: this host has no systemd" >&2; exit 1; }
command -v curl >/dev/null || { echo "install.sh: this host needs curl" >&2; exit 1; }

HERE="$(cd "$(dirname "$0")" && pwd)"
STATE=/var/lib/claktory-account
BIN=/usr/local/bin/claktory-account
CADDY_VERSION=2.11.9
say() { printf '%s\n' "$*"; }

# The service's user and folders.
if ! id claktory-account >/dev/null 2>&1; then
  useradd --system --home-dir "$STATE" --no-create-home --shell /usr/sbin/nologin claktory-account
fi
install -d -m 0700 -o claktory-account -g claktory-account "$STATE" "$STATE/mail" "$STATE/backups"
install -d -m 0755 /etc/claktory-account

# The binary, keeping the one before it to go back to.
if [ -x "$BIN" ]; then cp -p "$BIN" "$BIN.previous"; fi
install -m 0755 "$HERE/claktory-account" "$BIN.new"
mv -f "$BIN.new" "$BIN"

# The secrets: made once, never replaced.
FIRST=0
if [ ! -f "$STATE/secrets.json" ]; then
  runuser -u claktory-account -- "$BIN" init-secrets "$STATE/secrets.json" >/dev/null
  FIRST=1
fi

# DB-IP's free city file (CC BY 4.0), this month's or last month's.
if [ "$GEOIP" = 1 ]; then
  install -m 0755 "$HERE/update-geoip.sh" /usr/local/lib/claktory-account-update-geoip
  /usr/local/lib/claktory-account-update-geoip || say "The GeoIP file couldn't be fetched; places stay unknown until it is."
fi
GEOIP_OPTION=""
if [ "$GEOIP" = 1 ]; then GEOIP_OPTION="--geoip $STATE/geoip/city.mmdb"; fi

# Options, then the units.
OPTIONS="--listen 127.0.0.1:$PORT$ADMINS $GEOIP_OPTION"
[ -n "$MAIL_DOMAIN" ] && OPTIONS="$OPTIONS --mail-domain $MAIL_DOMAIN"
[ -n "$CAP" ] && OPTIONS="$OPTIONS --email-daily-cap $CAP"
printf 'CLAKTORY_ACCOUNT_OPTIONS=%s\n' "$OPTIONS" > /etc/claktory-account/options
for unit in claktory-account.service claktory-account-backup.service claktory-account-backup.timer \
  claktory-account-geoip.service claktory-account-geoip.timer; do
  install -m 0644 "$HERE/$unit" "/etc/systemd/system/$unit"
done
systemctl daemon-reload
systemctl enable claktory-account.service claktory-account-backup.timer >/dev/null 2>&1
if [ "$GEOIP" = 1 ]; then systemctl enable --now claktory-account-geoip.timer >/dev/null 2>&1; fi
systemctl start claktory-account-backup.timer
systemctl restart claktory-account.service

# It answers, or the binary before it goes back.
answers() {
  i=0
  while [ $i -lt 20 ]; do
    if curl -fsS "http://127.0.0.1:$PORT/.well-known/claktory-account" >/dev/null 2>&1; then return 0; fi
    i=$((i + 1)); sleep 0.5
  done
  return 1
}
if ! answers; then
  journalctl -u claktory-account -n 20 --no-pager >&2 || true
  if [ -x "$BIN.previous" ]; then
    mv -f "$BIN.previous" "$BIN"
    systemctl restart claktory-account.service
    echo "install.sh: the new service didn't answer, so the one before it is back" >&2
  else
    echo "install.sh: the service didn't answer" >&2
  fi
  exit 1
fi

# Caddy: installed when missing, from Caddy's own release, checked against
# its checksums.
if ! command -v caddy >/dev/null; then
  case "$(uname -m)" in
    x86_64) CADDY_ARCH=amd64 ;;
    aarch64 | arm64) CADDY_ARCH=arm64 ;;
    *) echo "install.sh: no Caddy for $(uname -m)" >&2; exit 1 ;;
  esac
  TMP="$(mktemp -d)"
  NAME="caddy_${CADDY_VERSION}_linux_${CADDY_ARCH}.tar.gz"
  URL="https://github.com/caddyserver/caddy/releases/download/v$CADDY_VERSION"
  curl -fsSL -o "$TMP/$NAME" "$URL/$NAME"
  curl -fsSL -o "$TMP/sums" "$URL/caddy_${CADDY_VERSION}_checksums.txt"
  (cd "$TMP" && grep " $NAME\$" sums | sha512sum -c - >/dev/null) || { echo "install.sh: Caddy's checksum doesn't match" >&2; exit 1; }
  tar -xzf "$TMP/$NAME" -C "$TMP" caddy
  install -m 0755 "$TMP/caddy" /usr/bin/caddy
  rm -rf "$TMP"
  if ! id caddy >/dev/null 2>&1; then
    useradd --system --home-dir /var/lib/caddy --create-home --shell /usr/sbin/nologin caddy
  fi
  install -m 0644 "$HERE/caddy.service" /etc/systemd/system/caddy.service
  systemctl daemon-reload
fi
install -d -m 0755 /etc/caddy

# The site, in a file of its own; a Caddyfile this kit didn't write gets an
# import line instead of being replaced.
TLS_LINE=""
[ "$TLS" = internal ] && TLS_LINE="	tls internal"
{
  printf '# Written by Claktory'"'"'s deploy kit (install.sh); it rewrites this file.\n'
  printf '%s {\n%s\n\tencode gzip\n\theader -Server\n\tredir / /account\n\treverse_proxy 127.0.0.1:%s\n}\n' "$DOMAIN" "$TLS_LINE" "$PORT"
  if [ -n "$REDIRECT" ]; then
    printf '%s {\n%s\n\tredir https://%s{uri}\n}\n' "$REDIRECT" "$TLS_LINE" "$DOMAIN"
  fi
} > /etc/caddy/claktory-account.caddy
if [ ! -f /etc/caddy/Caddyfile ] || grep -q "Claktory's deploy kit" /etc/caddy/Caddyfile; then
  printf "# Written by Claktory's deploy kit (install.sh).\nimport /etc/caddy/claktory-account.caddy\n" > /etc/caddy/Caddyfile
elif ! grep -q "claktory-account.caddy" /etc/caddy/Caddyfile; then
  printf '\nimport /etc/caddy/claktory-account.caddy\n' >> /etc/caddy/Caddyfile
fi
caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null 2>&1 || {
  caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >&2
  exit 1
}
systemctl enable caddy >/dev/null 2>&1
systemctl reload-or-restart caddy

KNOWN="$(curl -fsS "http://127.0.0.1:$PORT/.well-known/claktory-account")"
KEY="$(printf '%s' "$KNOWN" | sed -n 's/.*"service_key":"\([^"]*\)".*/\1/p')"
VERSION="$(printf '%s' "$KNOWN" | sed -n 's/.*"version":"\([^"]*\)".*/\1/p')"
say "claktory-account $VERSION is running at https://$DOMAIN (service key $KEY)."
if [ "$FIRST" = 1 ]; then say "NEW SECRETS: $STATE/secrets.json"; fi
if ! grep -q '"resend_api_key": "re' "$STATE/secrets.json"; then
  say "No Resend key yet: emails are written to $STATE/mail until there is one."
fi
