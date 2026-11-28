#!/usr/bin/env bash
# Asks this Mac's build service (claktory-ops mac/build-user) for a
# build or a signature, and waits for it. The service builds a commit as its
# own user, and signs the result with the Developer ID only its signer holds;
# every file it signs is a line in /Library/Claktory/build/signed.log.
#
#   scripts/build-request.sh release
#       Builds and signs main's commit as the next release of the real
#       Claktory (scripts/build-service.sh release). Checks first what
#       scripts/release.sh would: its changelog is stamped with a version
#       newer than ~/.factory/releases' latest, and no secret is in what
#       goes public (scripts/check-secrets.sh). Prints the request's NAME;
#       then scripts/release.sh --from-service NAME notarizes and publishes
#       it. A release build takes an hour or so.
#   scripts/build-request.sh dmg OF FILE
#       Signs the disk image FILE (scripts/make-dmg.sh makes it) as release
#       request OF's: the service signs it only if its Claktory.app is exactly
#       OF's signed app. Prints the signed copy's path.
#   scripts/build-request.sh wait NAME
#       Waits for request NAME, prints the end of its log, and fails unless
#       it succeeded.
#
# --no-wait (after release or dmg) only sends the request.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/lib.sh"
B=/Library/Claktory/build REQ=/Library/Claktory/build/requests OUT=/Library/Claktory/build/out

usage() { echo "usage: scripts/build-request.sh release [--no-wait] | dmg OF FILE [--no-wait] | wait NAME" >&2; exit 2; }
[[ -d "$REQ" && -w "$REQ" ]] || { echo "this Mac has no build service for this account ($REQ); see claktory-ops mac/build-user" >&2; exit 1; }

# Waits for NAME's result; the service starts when the requests folder
# changes, so it's nudged again if it hasn't started within two minutes.
wait_for() {
  local name="$1" waited=0 state
  while :; do
    state="$(cat "$OUT/$name/status" 2>/dev/null || true)"
    case "$state" in
      ok | failed*) break ;;
      "") if (( waited == 120 )); then rm -f "$REQ/.nudge"; : >"$REQ/.nudge"; fi ;;
    esac
    sleep 10; waited=$((waited + 10))
  done
  tail -n 15 "$OUT/$name/log" >&2 || true
  # The service has what it needs once it's done: the request's files go.
  rm -f "$REQ/$name.request" "$REQ/$name.bundle" "$REQ/$name.dmg"
  [[ "$state" == ok ]] || { echo "the build service's $name: $state" >&2; return 1; }
}

# Puts a request in place: its payload first, then the request itself,
# each written aside and renamed, so the service never reads half of one.
send() { # NAME PAYLOAD-EXTENSION PAYLOAD-FILE REQUEST-LINES
  local name="$1" ext="$2" payload="$3" lines="$4"
  [[ ! -e "$OUT/$name" && ! -e "$REQ/$name.request" ]] || { echo "$name was already asked for" >&2; return 1; }
  cp "$payload" "$REQ/.$name.$ext.partial"
  mv "$REQ/.$name.$ext.partial" "$REQ/$name.$ext"
  printf '%s\n' "$lines" >"$REQ/.$name.request.partial"
  mv "$REQ/.$name.request.partial" "$REQ/$name.request"
}

case "${1:-}" in
  release)
    WAIT=true; [[ "${2:-}" == --no-wait ]] && WAIT=false
    [[ $# -le 2 ]] || usage
    SHA="$(git -C "$ROOT" rev-parse main)"
    # What release.sh would refuse, refused now rather than in an hour.
    WORK="$(mktemp -d)"
    trap 'rm -rf "$WORK"' EXIT
    git -C "$ROOT" archive "$SHA" docs/CHANGELOG.md scripts/changelog.sh scripts/lib.sh scripts/toolchain.pins scripts/build-service.sh | tar -x -C "$WORK"
    [[ -f "$WORK/scripts/build-service.sh" ]] || { echo "main (${SHA:0:10}) has no scripts/build-service.sh" >&2; exit 1; }
    VERSION="$("$WORK/scripts/changelog.sh" pending "$HOME/.factory/releases")"
    "$ROOT/scripts/check-secrets.sh" "$SHA"
    NAME="release-$VERSION-${SHA:0:10}-$(date -u +%Y%m%d%H%M%S)"
    git -C "$ROOT" bundle create "$WORK/source.bundle" main 2>/dev/null
    git bundle verify -q "$WORK/source.bundle" 2>/dev/null || git -C "$ROOT" bundle verify -q "$WORK/source.bundle"
    send "$NAME" bundle "$WORK/source.bundle" "sha=$SHA
job=release"
    echo "asked the build service for $NAME (main, ${SHA:0:10}, $VERSION)" >&2
    if $WAIT; then wait_for "$NAME"; fi
    echo "$NAME"
    ;;
  dmg)
    OF="${2:-}" FILE="${3:-}"
    WAIT=true; [[ "${4:-}" == --no-wait ]] && WAIT=false
    [[ "$OF" =~ ^[A-Za-z0-9._-]{1,64}$ && -f "$FILE" && $# -le 4 ]] || usage
    [[ "$(cat "$OUT/$OF/status" 2>/dev/null)" == ok ]] || { echo "$OF isn't a finished release build of the service's" >&2; exit 1; }
    NAME="dmg-$(basename "$FILE" .dmg | tr -c 'A-Za-z0-9._-\n' '_' | cut -c1-36)-$(date -u +%Y%m%d%H%M%S)"
    send "$NAME" dmg "$FILE" "sign=dmg
of=$OF"
    echo "asked the build service to sign $(basename "$FILE") as $OF's (request $NAME)" >&2
    if $WAIT; then
      wait_for "$NAME"
      echo "$OUT/$NAME/files/$NAME.dmg"
    else
      echo "$NAME"
    fi
    ;;
  wait)
    [[ "${2:-}" =~ ^[A-Za-z0-9._-]{1,64}$ && $# -eq 2 ]] || usage
    wait_for "$2"
    ;;
  *) usage ;;
esac
