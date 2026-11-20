#!/usr/bin/env bash
# Tries scripts/deploy-account.sh end to end without any real host: a
# Debian container with systemd and ssh plays the VPS (with a sudo user,
# as most hosts give you), the service is deployed to it with Caddy in
# front (its own certificate authority, since there's no DNS), and then:
#
#   - a Claktory's whole round through Caddy over HTTPS: sign in with the
#     code from the service's mail folder, unlock, check in, endpoints,
#     public stats, the account page (tests/all/live.rs);
#   - an update (the same deploy again) keeps the secrets and the data;
#   - a binary that doesn't start is rolled back;
#   - after a restart of the "VPS" everything comes back;
#   - a backup, and copying it and the secrets off with the deploy script.
#
#   scripts/test-deploy-account.sh [--keep]    --keep leaves the container running
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENGINE="${FACTORY_CONTAINER:-podman}"
NAME=claktory-account-deploy-test
IMAGE=localhost/claktory-account-deploy-test
SSH_PORT=2242 HTTPS_PORT=8443
KEEP=0
[[ "${1:-}" == --keep ]] && KEEP=1

WORK="$(mktemp -d)"
cleanup() {
  if [[ $KEEP == 0 ]]; then "$ENGINE" rm -f "$NAME" >/dev/null 2>&1 || true; fi
  rm -rf "$WORK"
}
trap cleanup EXIT
step() { printf '\n== %s\n' "$*"; }
in_host() { "$ENGINE" exec "$NAME" sh -c "$1"; }

step "A Debian host with systemd and ssh"
cat > "$WORK/Containerfile" <<'EOF'
FROM docker.io/library/debian:12
RUN apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
      systemd systemd-sysv openssh-server curl ca-certificates sudo >/dev/null && rm -rf /var/lib/apt/lists/*
RUN systemctl enable ssh && useradd --create-home --shell /bin/bash matt \
    && echo 'matt ALL=(ALL) NOPASSWD: ALL' > /etc/sudoers.d/matt && mkdir -p /home/matt/.ssh
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
EOF
"$ENGINE" build -q -t "$IMAGE" "$WORK" >/dev/null
"$ENGINE" rm -f "$NAME" >/dev/null 2>&1 || true
"$ENGINE" run -d --name "$NAME" --systemd=always --hostname vps \
  -p "127.0.0.1:$SSH_PORT:22" -p "127.0.0.1:$HTTPS_PORT:443" "$IMAGE" >/dev/null
ssh-keygen -q -t ed25519 -N "" -f "$WORK/key"
"$ENGINE" cp "$WORK/key.pub" "$NAME:/home/matt/.ssh/authorized_keys"
in_host "chown -R matt:matt /home/matt/.ssh && chmod 700 /home/matt/.ssh && chmod 600 /home/matt/.ssh/authorized_keys"
cat > "$WORK/ssh_config" <<EOF
Host vps
  HostName 127.0.0.1
  Port $SSH_PORT
  User matt
  IdentityFile $WORK/key
  IdentitiesOnly yes
  StrictHostKeyChecking no
  UserKnownHostsFile /dev/null
  LogLevel ERROR
EOF
for _ in $(seq 1 30); do ssh -F "$WORK/ssh_config" vps true 2>/dev/null && break; sleep 1; done

DEPLOY=("$ROOT/scripts/deploy-account.sh" --ssh-config "$WORK/ssh_config" --to "$WORK/saved")
INSTALL=(--domain account.test --tls-internal --admin-email ops@example.com)

step "Deploy"
"${DEPLOY[@]}" "${INSTALL[@]}" vps install
[[ -s "$WORK/saved/secrets.json" ]] || { echo "the new secrets weren't copied here" >&2; exit 1; }
KEY_BEFORE="$(in_host "cat /var/lib/claktory-account/secrets.json" | sed -n 's/.*"signing_key": "\([^"]*\)".*/\1/p')"
in_host "stat -c '%a %U' /var/lib/claktory-account/secrets.json" | grep -qx "600 claktory-account"
in_host "systemctl is-enabled claktory-account caddy claktory-account-backup.timer claktory-account-geoip.timer" >/dev/null
in_host "ls -la /var/lib/claktory-account/geoip/city.mmdb"
if in_host "journalctl -u claktory-account --no-pager" | grep -q "no GeoIP file"; then echo "the GeoIP file isn't in use" >&2; exit 1; fi

"$ENGINE" cp "$NAME:/var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt" "$WORK/caddy-root.crt"
live() {
  CLAKTORY_ACCOUNT_LIVE="https://account.test:$HTTPS_PORT" \
  CLAKTORY_ACCOUNT_RESOLVE="account.test:$HTTPS_PORT=127.0.0.1:$HTTPS_PORT" \
  CLAKTORY_ACCOUNT_CA="$WORK/caddy-root.crt" \
  CLAKTORY_ACCOUNT_MAIL="$ENGINE exec $NAME sh -c 'cat \"\$(ls -1t /var/lib/claktory-account/mail/* | head -n 1)\"'" \
    cargo test --manifest-path "$ROOT/Cargo.toml" -q -p claktory-account --test all live:: -- --nocapture 2>&1 | grep -E "service key|signed in|public stats|test result|panicked|assert" 
}

step "A Claktory's round through Caddy"
live
curl -sS --cacert "$WORK/caddy-root.crt" --resolve "account.test:$HTTPS_PORT:127.0.0.1" -o /dev/null -w "https://account.test/ answers %{http_code}, to %{redirect_url}\n" "https://account.test:$HTTPS_PORT/"

step "An update keeps the secrets and the data"
"${DEPLOY[@]}" "${INSTALL[@]}" vps install | tail -n 2
[[ "$(in_host "cat /var/lib/claktory-account/secrets.json" | sed -n 's/.*"signing_key": "\([^"]*\)".*/\1/p')" == "$KEY_BEFORE" ]] || { echo "the secrets changed" >&2; exit 1; }
in_host "runuser -u claktory-account -- claktory-account stats --db /var/lib/claktory-account/account.db" | sed -n '1,4p'

step "A binary that doesn't start is rolled back"
STAGE="$ROOT/target/account-deploy/$(ssh -F "$WORK/ssh_config" vps uname -m | sed 's/arm64/aarch64/')"
"$ENGINE" exec "$NAME" mkdir -p /tmp/broken
for file in "$STAGE"/*; do "$ENGINE" cp "$file" "$NAME:/tmp/broken/"; done
in_host 'printf "#!/bin/sh\nexit 1\n" > /tmp/broken/claktory-account && chmod +x /tmp/broken/claktory-account'
if in_host "sh /tmp/broken/install.sh --domain account.test --tls-internal --no-geoip" >/dev/null 2>"$WORK/rollback.log"; then
  echo "a broken binary was installed" >&2; exit 1
fi
grep -q "the one before it is back" "$WORK/rollback.log"
in_host "curl -fsS http://127.0.0.1:8417/.well-known/claktory-account" >/dev/null && echo "rolled back, and it answers"
"${DEPLOY[@]}" "${INSTALL[@]}" vps install | tail -n 1

step "After a restart of the host"
"$ENGINE" restart "$NAME" >/dev/null
for _ in $(seq 1 30); do in_host "curl -fsS http://127.0.0.1:8417/.well-known/claktory-account" >/dev/null 2>&1 && break; sleep 1; done
for _ in $(seq 1 30); do ssh -F "$WORK/ssh_config" vps true 2>/dev/null && break; sleep 1; done
live

step "The Resend key"
printf 're_test_key_123\n' | "${DEPLOY[@]}" vps resend-key
in_host "grep -q '\"resend_api_key\": \"re_test_key_123\"' /var/lib/claktory-account/secrets.json"
in_host "stat -c '%a %U' /var/lib/claktory-account/secrets.json" | grep -qx "600 claktory-account"
for _ in $(seq 1 20); do in_host "curl -fsS http://127.0.0.1:8417/.well-known/claktory-account" >/dev/null 2>&1 && break; sleep 0.5; done
in_host "journalctl -u claktory-account -n 5 --no-pager" | grep -q "listening on" && echo "the key is in, and the service restarted with it"

step "Backups"
in_host "systemctl start claktory-account-backup.service && ls /var/lib/claktory-account/backups"
"${DEPLOY[@]}" vps backup
ls "$WORK/saved"
"${DEPLOY[@]}" vps status | sed -n '1,3p'

step "All good"
