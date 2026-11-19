#!/usr/bin/env bash
# Pushes Factory's source, the main branch as committed, to its public
# repository (scripts/publish.sh runs it for each release). Only from main, in
# a clean checkout, and only once the secret check (scripts/check-secrets.sh)
# has passed on the whole history it pushes.
#
# It never force-pushes, with one exception made once: a repository whose main
# is still the lone commit GitHub makes for a new repository (README.md, no
# parent) is replaced by Factory's history, with a lease on that commit. Any
# other main this one doesn't contain is refused; sort it out by hand.
#
#   scripts/push-source.sh URL [--expect SHA] [--dry-run]
#
# URL: the repository (https://github.com/OWNER/NAME.git, pushed with gh's
# sign-in, or a path to try it on a bare repository). --expect: refuse unless
# main is that commit (what a release was built from). --dry-run: check
# everything and say what it would push, without pushing.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
URL="" EXPECT="" DRY=false
usage() { echo "usage: scripts/push-source.sh URL [--expect SHA] [--dry-run]" >&2; exit 2; }
while (($#)); do
  case "$1" in
    --expect) EXPECT="${2:-}"; shift 2 || usage ;;
    --dry-run) DRY=true; shift ;;
    -*) usage ;;
    *) [[ -z "$URL" ]] || usage; URL="$1"; shift ;;
  esac
done
[[ -n "$URL" ]] || usage
git() { command git -C "$ROOT" "$@"; }

[[ "$(git symbolic-ref -q --short HEAD || true)" == "main" ]] || {
  echo "the source is pushed from main only; this checkout is on $(git symbolic-ref -q --short HEAD || git rev-parse --short HEAD)" >&2
  exit 1
}
[[ -z "$(git status --porcelain)" ]] || {
  echo "the checkout has uncommitted or untracked changes; the source is pushed from a clean tree only:" >&2
  git status --short >&2
  exit 1
}
SHA="$(git rev-parse HEAD)"
[[ -z "$EXPECT" || "$SHA" == "$(git rev-parse "$EXPECT")" ]] || {
  echo "main moved on to $SHA since the release was built from $EXPECT; not pushing" >&2
  exit 1
}

"$ROOT/scripts/check-secrets.sh" "$SHA"

# gh's sign-in pushes to GitHub, whatever git's own credentials are.
auth=()
[[ "$URL" == https://github.com/* ]] && auth=(-c credential.helper= -c "credential.helper=!gh auth git-credential")

REMOTE="$(git ${auth[@]+"${auth[@]}"} ls-remote "$URL" refs/heads/main | cut -f1)"
LEASE=()
if [[ -z "$REMOTE" ]]; then
  what="main, first push"
elif [[ "$REMOTE" == "$SHA" ]]; then
  echo "the source at $URL is already $SHA"
  exit 0
elif git cat-file -e "$REMOTE^{commit}" 2>/dev/null && git merge-base --is-ancestor "$REMOTE" "$SHA"; then
  what="main, fast-forward from ${REMOTE:0:10}"
else
  # Fetch only its objects (no refs, no FETCH_HEAD) to see what it is.
  git ${auth[@]+"${auth[@]}"} fetch -q --no-tags --no-write-fetch-head "$URL" refs/heads/main
  if [[ "$(git rev-list --count "$REMOTE")" == 1 && "$(git ls-tree --name-only "$REMOTE")" == "README.md" ]]; then
    what="main, replacing the repository's first README-only commit ${REMOTE:0:10} (once)"
    LEASE=(--force-with-lease="refs/heads/main:$REMOTE")
  else
    echo "main at $URL is ${REMOTE:0:10}, which this main doesn't contain; not overwriting it" >&2
    exit 1
  fi
fi

if $DRY; then
  echo "would push $SHA to $URL: $what"
  exit 0
fi
git ${auth[@]+"${auth[@]}"} push ${LEASE[@]+"${LEASE[@]}"} "$URL" "$SHA:refs/heads/main"
echo "pushed $SHA to $URL: $what"
