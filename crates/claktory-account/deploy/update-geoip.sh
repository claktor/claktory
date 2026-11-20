#!/bin/sh
# Fetches DB-IP's free "IP to City Lite" file (CC BY 4.0, a new one each
# month) for Claktory's account service, and restarts the service when it
# changed. Run by install.sh and monthly by claktory-account-geoip.timer.
set -eu
DIR=/var/lib/claktory-account/geoip
install -d -m 0700 -o claktory-account -g claktory-account "$DIR"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
for month in "$(date -u +%Y-%m)" "$(date -u -d "$(date -u +%Y-%m-15) -1 month" +%Y-%m 2>/dev/null || true)"; do
  [ -n "$month" ] || continue
  if curl -fsSL -o "$TMP/city.mmdb.gz" "https://download.db-ip.com/free/dbip-city-lite-$month.mmdb.gz"; then
    gunzip -t "$TMP/city.mmdb.gz" && gunzip "$TMP/city.mmdb.gz" && break
  fi
  rm -f "$TMP/city.mmdb.gz"
done
[ -s "$TMP/city.mmdb" ] || { echo "no GeoIP file could be fetched from DB-IP" >&2; exit 1; }
if [ -f "$DIR/city.mmdb" ] && cmp -s "$TMP/city.mmdb" "$DIR/city.mmdb"; then
  echo "the GeoIP file is up to date"
  exit 0
fi
# A new file, renamed into place: the service maps the old one until it restarts.
install -m 0600 -o claktory-account -g claktory-account "$TMP/city.mmdb" "$DIR/city.mmdb.new"
mv -f "$DIR/city.mmdb.new" "$DIR/city.mmdb"
echo "fetched DB-IP's GeoIP city file"
if systemctl is-active --quiet claktory-account.service; then systemctl restart claktory-account.service; fi
