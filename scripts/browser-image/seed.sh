#!/bin/bash
# Run once while the image is built, as root: the image's own Chrome, under
# its own policy, fetches Safe Browsing's lists into a fresh user-data-dir,
# and they're kept as the seed every new profile starts from, so a profile's
# first minutes aren't spent without them. Fails when the lists haven't come
# after WAIT seconds.
set -euo pipefail
SEED=/usr/local/share/claktory-browser/safe-browsing
WAIT="${WAIT:-600}"
die() { echo "seed.sh: $*" >&2; exit 1; }
[[ -s /etc/opt/chrome/policies/managed/claktory.json ]] || die "Chrome's policy isn't in the image"
work="$(mktemp -d)"
sb="$work/profile/Safe Browsing"
Xvfb :91 -nolisten tcp -nolisten local -screen 0 1280x800x24 >/dev/null 2>&1 &
xvfb=$!
# Root during the build, so no sandbox: this Chrome opens nothing but about:blank.
DISPLAY=:91 google-chrome-stable --user-data-dir="$work/profile" --no-sandbox --no-first-run --no-default-browser-check \
  --disable-sync --password-store=basic about:blank >"$work/chrome.log" 2>&1 &
chrome=$!
# Ready: the phishing and malware lists each with a data file of their own,
# and nothing in the folder changed for 20 seconds.
has() { compgen -G "$sb/$1.store" >/dev/null && find "$sb" -maxdepth 1 -type f -name "$1.store.*_*" -size +0 | grep -q .; }
waited=0 last="" still=0
while :; do
  if has UrlSoceng && has UrlMalware; then
    now="$(ls -l --time-style=+%s "$sb" | md5sum)"
    if [[ "$now" == "$last" ]]; then still=$((still + 5)); else still=0 last="$now"; fi
    ((still < 20)) || break
  fi
  kill -0 "$chrome" 2>/dev/null || { tail -20 "$work/chrome.log" >&2; die "Chrome ended before Safe Browsing's lists came"; }
  ((waited < WAIT)) || { ls -la "$sb" >&2 2>/dev/null || true; die "Safe Browsing's lists didn't come within $WAIT seconds"; }
  sleep 5; waited=$((waited + 5))
done
kill -TERM "$chrome"
for _ in $(seq 30); do kill -0 "$chrome" 2>/dev/null || break; sleep 1; done
! kill -0 "$chrome" 2>/dev/null || die "Chrome didn't end"
kill "$xvfb" 2>/dev/null || true
has UrlSoceng && has UrlMalware || die "the lists went when Chrome ended"
install -d -o root -g root -m 0755 "$SEED"
find "$sb" -maxdepth 1 -type f -name '*.store*' -exec install -o root -g root -m 0644 {} "$SEED/" \;
rm -rf "$work"
echo "seed.sh: Safe Browsing's lists after ${waited}s: $(ls "$SEED" | wc -l) files, $(du -sk "$SEED" | cut -f1) KB" >&2
