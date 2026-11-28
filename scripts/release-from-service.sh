#!/usr/bin/env bash
# The second half of a real release on a Mac with the build service
# (claktory-ops mac/build-user): takes the release the service built and
# signed (scripts/build-request.sh release), checks it against what the
# service recorded, notarizes and staples it here, and publishes it into the
# releases folder, as scripts/release.sh does with a release it signs itself.
# scripts/release.sh --from-service NAME [--out DIR] runs it.
#
#   scripts/release-from-service.sh NAME RELEASES_DIR
#
# The service's results are root's, in /Library/Claktory/build/out/NAME:
# status, files/ (the signed release), SHA256SUMS and source (the commit and
# its tree), and every signed file is a line in signed.log, which only root
# can append to. Nothing here can sign: it refuses a release the service
# didn't sign, a commit that isn't on main (main's own, for the person's
# releases), and anything that isn't exactly what the service recorded. With
# --out DIR (a rehearsal), CLAKTORY_BUILD_DIR may name another service folder.
#
# A backend whose source didn't change since the last release is reused, as
# release.sh does: the release keeps the last one's signed backend (and its
# backend-version), so updating to it only replaces the app. The app's own
# copy in Contents/Helpers, sealed in its signature, stays this build's, from
# the same source.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/lib.sh"

NAME="${1:-}" RELEASES="${2:-}"
[[ "$NAME" =~ ^[A-Za-z0-9._-]{1,64}$ && -n "$RELEASES" ]] || { echo "usage: scripts/release-from-service.sh NAME RELEASES_DIR" >&2; exit 2; }
REAL=false
[[ "$RELEASES" == "$HOME/.factory/releases" ]] && REAL=true
B=/Library/Claktory/build
if ! $REAL && [[ -n "${CLAKTORY_BUILD_DIR:-}" ]]; then B="$CLAKTORY_BUILD_DIR"; fi
O="$B/out/$NAME"
MANIFEST_URL="https://github.com/claktory/claktory/releases/latest/download/latest.json"

# For the person's releases, the team is pinned (CLAKTORY_TEAM_ID), and what
# the service recorded must be root's alone, as setup made it: its folders and
# files root's and writable by no one else, signed.log append-only.
if $REAL; then
  [[ "${CLAKTORY_TEAM_ID:-}" =~ ^[A-Z0-9]{10}$ ]] || { echo "a release needs CLAKTORY_TEAM_ID, the team its signature must be" >&2; exit 1; }
  for f in "$B" "$B/out" "$O" "$O/files" "$O/status" "$O/SHA256SUMS" "$O/source" "$B/signed.log"; do
    read -r uid mode < <(stat -f '%u %Lp' "$f" 2>/dev/null || echo "x 777")
    [[ "$uid" == 0 && ! -L "$f" ]] && (( (8#$mode & 8#022) == 0 )) || { echo "$f isn't root's alone (owner $uid, mode $mode): not trusting it" >&2; exit 1; }
  done
  [[ " $(stat -f '%Sf' "$B/signed.log" | tr ',' ' ') " == *" sappnd "* ]] || { echo "$B/signed.log isn't append-only: not trusting it" >&2; exit 1; }
fi
[[ "$(cat "$O/status" 2>/dev/null)" == ok ]] || { echo "the build service's $NAME isn't a finished build: $(cat "$O/status" 2>/dev/null || echo "no $O")" >&2; exit 1; }
SHA="$(sed -n 's/^sha=\([0-9a-f]\{40\}\)$/\1/p' "$O/source")"
TREE="$(sed -n 's/^tree=\([0-9a-f]\{40\}\)$/\1/p' "$O/source")"
grep -qx 'kind=release' "$O/source" && [[ -n "$SHA" && -n "$TREE" ]] || { echo "$NAME isn't a release build" >&2; exit 1; }

# The commit: on main, its tree the one the service built, and for the
# person's releases main's own commit.
git -C "$ROOT" merge-base --is-ancestor "$SHA" main 2>/dev/null || { echo "$NAME was built from $SHA, which isn't on main" >&2; exit 1; }
[[ "$(git -C "$ROOT" rev-parse "$SHA^{tree}")" == "$TREE" ]] || { echo "$NAME's tree isn't $SHA's" >&2; exit 1; }
if $REAL && [[ "$SHA" != "$(git -C "$ROOT" rev-parse main)" ]]; then
  echo "a release is built from main's commit only; $NAME is ${SHA:0:10}, main $(git -C "$ROOT" rev-parse --short main)" >&2
  exit 1
fi

# Exactly the files the service signed and recorded: every file listed in
# SHA256SUMS with its hash, none left out, and each a line in signed.log.
WORK="$(mktemp -d)"
partial=""
trap 'rm -rf "$WORK"; [[ -z "$partial" ]] || rm -rf "$partial"' EXIT
(cd "$O/files" && find . -type f | LC_ALL=C sort) >"$WORK/files"
sed 's/^[0-9a-f]\{64\}  //' "$O/SHA256SUMS" | LC_ALL=C sort >"$WORK/listed"
cmp -s "$WORK/files" "$WORK/listed" || { echo "$NAME's files aren't the ones its SHA256SUMS lists" >&2; diff "$WORK/listed" "$WORK/files" | sed -n 1,10p >&2; exit 1; }
[[ -z "$(find "$O/files" ! -type f ! -type d ! -type l -print -quit)" ]] || { echo "$NAME holds special files" >&2; exit 1; }
(cd "$O/files" && shasum -a 256 -c --quiet "$O/SHA256SUMS") || { echo "$NAME's files don't match its SHA256SUMS" >&2; exit 1; }
grep -F " id=$NAME kind=release sha=$SHA tree=$TREE " "$B/signed.log" | sed -n 's/.* sha256=\([0-9a-f]\{64\}\) file=\(.*\)$/\1  .\/\2/p' | LC_ALL=C sort -k2 >"$WORK/logged"
LC_ALL=C sort -k2 "$O/SHA256SUMS" | cmp -s - "$WORK/logged" || { echo "signed.log doesn't record $NAME's files as they are" >&2; exit 1; }

# The version: the one the commit's changelog is stamped with, newer than
# the last release here.
git -C "$ROOT" archive "$SHA" docs/CHANGELOG.md scripts/changelog.sh scripts/lib.sh | tar -x -C "$WORK"
VERSION="$("$WORK/scripts/changelog.sh" pending "$RELEASES")"
[[ "$(cat "$O/files/version" 2>/dev/null)" == "$VERSION" ]] || { echo "$NAME was built as $(cat "$O/files/version" 2>/dev/null || echo "no version"), not $VERSION" >&2; exit 1; }
[[ "$(cat "$O/files/update-url" 2>/dev/null)" == "$MANIFEST_URL" ]] || { echo "$NAME's backend checks another update channel" >&2; exit 1; }
[[ "$(cat "$O/files/source" 2>/dev/null)" == "$SHA" ]] || { echo "$NAME says it was built from another commit" >&2; exit 1; }

mkdir -p "$RELEASES"
partial="$RELEASES/$VERSION.partial"
rm -rf "$partial"
ditto "$O/files" "$partial"
rm -f "$partial/version"
APP="$partial/Factory.app"
[[ -d "$APP" && -f "$partial/factoryd" ]] || { echo "$NAME isn't a Claktory release" >&2; exit 1; }
if $REAL && [[ -e "$APP/Contents/Helpers/claktory-browser-host" ]]; then
  echo "$NAME carries the browser helper, which is for rehearsals until browser control passes B6b" >&2; exit 1
fi

# This Mac's checks again, with this account's names: the build ran as
# another user, who can't know them.
check_no_traces "$partial"
check_no_qa_provider "$partial/factoryd" "$APP/Contents/Helpers/factoryd" || exit 1
python3 "$ROOT/scripts/check-macho.py" "$APP/Contents/Frameworks" || { echo "the app's libraries wouldn't load (above); not releasing it" >&2; exit 1; }

# Signed by a Developer ID (CLAKTORY_TEAM_ID's team when it's set): the
# backend fingerprint counts who signed it, as release.sh's does.
IDENTITY="$(codesign -dvv "$partial/factoryd" 2>&1 | sed -n 's/^Authority=\(Developer ID Application: .*\)$/\1/p' | head -1)"
[[ -n "$IDENTITY" ]] || { echo "$NAME's backend isn't signed with a Developer ID" >&2; exit 1; }
if [[ -n "${CLAKTORY_TEAM_ID:-}" ]]; then
  for code in "$partial/factoryd" "$APP"; do
    [[ "$(codesign -dvv "$code" 2>&1 | sed -n 's/^TeamIdentifier=//p')" == "$CLAKTORY_TEAM_ID" ]] || { echo "${code#"$partial"/} isn't signed by team CLAKTORY_TEAM_ID" >&2; exit 1; }
  done
fi
[[ "$(cat "$partial/backend-fingerprint")" =~ ^[0-9a-f]{64}$ ]] || { echo "$NAME has no backend fingerprint" >&2; exit 1; }
FINGERPRINT="$(printf '%s\nsigning: %s\n' "$(cat "$partial/backend-fingerprint")" "developer-id $IDENTITY" | shasum -a 256 | cut -d' ' -f1)"
echo "$FINGERPRINT" >"$partial/backend-fingerprint"
PREVIOUS=""
[[ ! -f "$RELEASES/latest" ]] || PREVIOUS="$RELEASES/$(cat "$RELEASES/latest")"
if [[ -n "$PREVIOUS" && -f "$PREVIOUS/factoryd" && -f "$PREVIOUS/claktory-apps" && -f "$PREVIOUS/claktory-speech" && -f "$PREVIOUS/backend-version" && "$(cat "$PREVIOUS/backend-fingerprint" 2>/dev/null)" == "$FINGERPRINT" ]] &&
   { [[ ! -f "$partial/claktory-video-preview" ]] || [[ -f "$PREVIOUS/claktory-video-preview" ]]; } && { [[ ! -d "$partial/tools" ]] || [[ -d "$PREVIOUS/tools" ]]; }; then
  for item in factoryd claktory-apps claktory-speech claktory-video-preview tools backend-version; do
    [[ -e "$partial/$item" ]] || continue
    rm -rf "${partial:?}/$item"
    ditto "$PREVIOUS/$item" "$partial/$item"
  done
  echo "backend unchanged since $(cat "$partial/backend-version"); this release only updates the app"
fi
# Which build it was, for scripts/make-dmg.sh: the service signs its disk
# image only as this release's.
echo "$NAME" >"$partial/build-request"

# Notarized and stapled here, with the person's App Store Connect key.
"$ROOT/scripts/sign-release.sh" "$partial" --notarize-only
spctl --assess --type execute "$APP" || { echo "$APP isn't notarized; not releasing it" >&2; exit 1; }

mv "$partial" "$RELEASES/$VERSION"
partial=""
echo "$VERSION" >"$RELEASES/latest.new"
mv -f "$RELEASES/latest.new" "$RELEASES/latest"
# Keep the three newest, by version, as release.sh does.
ls -1 "$RELEASES" | grep -v '^latest' | while read -r old; do
  printf '%s\t%s\n' "$(version_key "$old")" "$old"
done | sort -r | cut -f2 | tail -n +4 | while read -r old; do
  rm -rf "${RELEASES:?}/$old"
done
echo "published Claktory $VERSION (the build service's $NAME, ${SHA:0:10}) to $RELEASES"
