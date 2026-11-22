#!/usr/bin/env bash
# Makes a release's source public: one commit per release in the public
# repository (scripts/publish.sh runs it for each release). That commit's
# files are exactly the release's source, main as committed here; its parent
# is the previous release's public commit; its message is what the release
# brings, from the changelog (public_commit in scripts/lib.sh). The
# day-to-day commits never leave this Mac, except to the private backup.
#
# Only from main, in a clean checkout. The secret check (scripts/check-secrets.sh)
# runs twice: on main's whole history, and on what goes public, the public
# history with the new commit and its files.
#
# The branch `public` here is what the repository's main was after the last
# push. GitHub's main only ever moves forward from it: anything else there is
# refused, to sort out by hand. One exception: a new repository's lone
# README-only first commit is replaced (once, with a lease on it).
#
#   scripts/push-source.sh URL [--version vX.Y.Z] [--source SHA] [--expect SHA] [--dry-run]
#
# URL: the repository (https://github.com/OWNER/NAME.git, pushed with gh's
# sign-in, or a path to try it on a bare repository). --version: the release
# (by default the newest in that commit's docs/CHANGELOG.md). --source: the
# commit whose files are published, a release's own source, which must be on
# main (main itself by default). --expect: refuse unless main is that commit.
# --dry-run: check everything and say what it would push, without pushing.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/lib.sh"
URL="" EXPECT="" DRY=false VERSION="" FROM=""
usage() { echo "usage: scripts/push-source.sh URL [--version vX.Y.Z] [--source SHA] [--expect SHA] [--dry-run]" >&2; exit 2; }
while (($#)); do
  case "$1" in
    --version) VERSION="${2:-}"; shift 2 || usage ;;
    --expect) EXPECT="${2:-}"; shift 2 || usage ;;
    --source) FROM="${2:-}"; shift 2 || usage ;;
    --dry-run) DRY=true; shift ;;
    -*) usage ;;
    *) [[ -z "$URL" ]] || usage; URL="$1"; shift ;;
  esac
done
[[ -n "$URL" ]] || usage
git() { command git -C "$ROOT" "$@"; }

[[ "$(git symbolic-ref -q --short HEAD || true)" == "main" ]] || {
  echo "the source is published from main only; this checkout is on $(git symbolic-ref -q --short HEAD || git rev-parse --short HEAD)" >&2
  exit 1
}
[[ -z "$(git status --porcelain)" ]] || {
  echo "the checkout has uncommitted or untracked changes; the source is published from a clean tree only:" >&2
  git status --short >&2
  exit 1
}
SHA="$(git rev-parse HEAD)"
[[ -z "$EXPECT" || "$SHA" == "$(git rev-parse "$EXPECT")" ]] || {
  echo "main moved on to $SHA since the release was built from $EXPECT; not pushing" >&2
  exit 1
}
if [[ -n "$FROM" ]]; then
  FROM="$(git rev-parse --verify -q "$FROM^{commit}")" && git merge-base --is-ancestor "$FROM" "$SHA" || {
    echo "the source to publish isn't a commit on main; not pushing" >&2
    exit 1
  }
  SHA="$FROM"
fi
[[ -n "$VERSION" ]] || VERSION="$(git show "$SHA:docs/CHANGELOG.md" | sed -n 's/^## \(v[0-9][0-9.]*\)$/\1/p' | head -1)"
[[ -n "$VERSION" ]] || { echo "${SHA:0:10}'s docs/CHANGELOG.md has no released version" >&2; exit 1; }

"$ROOT/scripts/check-secrets.sh" "$SHA"

# gh's sign-in pushes to GitHub, whatever git's own credentials are.
auth=()
[[ "$URL" == https://github.com/* ]] && auth=(-c credential.helper= -c "credential.helper=!gh auth git-credential")

REMOTE="$(git ${auth[@]+"${auth[@]}"} ls-remote "$URL" refs/heads/main | cut -f1)"
LOCAL="$(git rev-parse -q --verify refs/heads/public || true)"
LEASE=() PARENT=""
if [[ -n "$REMOTE" ]]; then
  # Its objects only (no refs, no FETCH_HEAD), to build on it and check it.
  git ${auth[@]+"${auth[@]}"} fetch -q --no-tags --no-write-fetch-head "$URL" refs/heads/main
fi
if [[ -z "$REMOTE" ]]; then
  what="main, the first release"
elif [[ "$(git rev-list --count "$REMOTE")" == 1 && "$(git ls-tree --name-only "$REMOTE")" == "README.md" ]]; then
  what="main, replacing the repository's first README-only commit ${REMOTE:0:10} (once)"
  LEASE=(--force-with-lease="refs/heads/main:$REMOTE")
elif [[ -n "$LOCAL" && "$LOCAL" != "$REMOTE" ]]; then
  echo "main at $URL is ${REMOTE:0:10}, but the last release published from here left it at ${LOCAL:0:10} (branch public); not overwriting it" >&2
  exit 1
else
  PARENT="$REMOTE"
  what="main, after ${REMOTE:0:10} ($(git log -1 --format=%s "$REMOTE"))"
fi

if [[ -n "$PARENT" && "$(git rev-parse "$PARENT^{tree}")" == "$(git rev-parse "$SHA^{tree}")" ]]; then
  git update-ref refs/heads/public "$PARENT"
  echo "the source at $URL is already $SHA's, as ${PARENT:0:10}"
  exit 0
fi

NEW="$(public_commit "$ROOT" "$SHA" "$VERSION" "$PARENT")"
"$ROOT/scripts/check-secrets.sh" "$NEW"

if $DRY; then
  echo "would push $VERSION, ${NEW:0:10} with ${SHA:0:10}'s files, to $URL: $what"
  exit 0
fi
git ${auth[@]+"${auth[@]}"} push ${LEASE[@]+"${LEASE[@]}"} "$URL" "$NEW:refs/heads/main"
git update-ref refs/heads/public "$NEW"
echo "pushed $VERSION as $NEW, with ${SHA:0:10}'s files, to $URL: $what"
