#!/usr/bin/env bash
# docs/CHANGELOG.md says what each version added and fixed, newest first.
# Changes add lines under "## Unreleased"; a release turns them into a version.
#
#   scripts/changelog.sh stamp [--incompatible]
#       Before a release: names the Unreleased entries with the next version
#       ("## v0.1.4", with a "Released <when> UTC." line under it) and starts a
#       new, empty Unreleased. Commit it, then run scripts/release.sh (or
#       publish.sh), which builds that version. Versions count up the last
#       number (v0.1.3, v0.1.4…). --incompatible goes to the next middle number
#       (v0.1.4 -> v0.2.0): only when this version's app and an older backend,
#       or this backend and an older app, can't work together, such as a
#       protocol change an older app can't follow. See AGENTS.md.
#   scripts/changelog.sh pending RELEASES_DIR
#       For release.sh: prints the version to build, the newest one in the
#       changelog, if it has entries and is newer than RELEASES_DIR/latest.
#       Fails, saying what to do, when there's nothing new to release.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/lib.sh"
CHANGELOG="$ROOT/docs/CHANGELOG.md"

[[ -f "$CHANGELOG" ]] || { echo "$CHANGELOG is missing" >&2; exit 1; }

# The entries ("- …" lines) under heading $1.
entries() {
  awk -v heading="$1" '/^## /{inside = (substr($0, 4) == heading); next} inside && /^- /{n++} END{print n + 0}' "$CHANGELOG"
}

# The newest version heading, after Unreleased.
newest() {
  awk '/^## /{heading = substr($0, 4); if (heading != "Unreleased") {print heading; exit}}' "$CHANGELOG"
}

case "${1:-}" in
  stamp)
    case "${2:-}" in
      "") kind="" ;;
      --incompatible) kind=incompatible ;;
      *) echo "usage: scripts/changelog.sh stamp [--incompatible]" >&2; exit 2 ;;
    esac
    grep -q '^## Unreleased$' "$CHANGELOG" || { echo "docs/CHANGELOG.md has no \"## Unreleased\" heading" >&2; exit 1; }
    if [[ "$(entries Unreleased)" == 0 ]]; then
      echo "Nothing to stamp: add what changed under \"## Unreleased\" in docs/CHANGELOG.md first." >&2
      exit 1
    fi
    version="$(next_version $kind)"
    released="Released $(date -u '+%Y-%m-%d %H:%M') UTC."
    awk -v version="$version" -v released="$released" '/^## Unreleased$/ && !done {print; print ""; print "## " version; print released; done = 1; next} {print}' "$CHANGELOG" >"$CHANGELOG.new"
    mv "$CHANGELOG.new" "$CHANGELOG"
    echo "$version"
    echo "Stamped the Unreleased entries as $version. Commit docs/CHANGELOG.md, then release." >&2
    ;;
  pending)
    releases="${2:?usage: scripts/changelog.sh pending RELEASES_DIR}"
    latest="$(cat "$releases/latest" 2>/dev/null || true)"
    version="$(newest)"
    if [[ -n "$version" && "$version" =~ ^v?[0-9][0-9A-Za-z.-]*$ ]] && version_newer "$version" "$latest" && [[ "$(entries "$version")" != 0 ]]; then
      echo "$version"
      exit 0
    fi
    if [[ "$(entries Unreleased)" != 0 ]]; then
      echo "docs/CHANGELOG.md has entries under \"## Unreleased\": run scripts/changelog.sh stamp, commit it, then release." >&2
    else
      echo "Nothing in docs/CHANGELOG.md for a new release (the newest version, ${version:-none}, is already released): add what changed under \"## Unreleased\", run scripts/changelog.sh stamp, commit it, then release." >&2
    fi
    exit 1
    ;;
  *)
    echo "usage: scripts/changelog.sh stamp [--incompatible] | pending RELEASES_DIR" >&2
    exit 2
    ;;
esac
