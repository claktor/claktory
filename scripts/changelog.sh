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
#   scripts/changelog.sh summary [VERSION [FILE]]
#       Prints VERSION's summary (the newest version's by default; FILE
#       instead of docs/CHANGELOG.md), for publish.sh, which puts it at the
#       top of the GitHub release notes. Fails when there's none, or more
#       than one.
#
# The summary (Trish 40653): one plain line, not a "- " item, directly under
# a version's "Released … UTC." line. It's written as the first line under
# "## Unreleased", and stamp keeps it in place under the new version
# (stamp --summary "…" sets or replaces it). The app's What's new leaves it
# out; it's for the release page, and for people who find Claktory there.
#   - One sentence, at most 25 words.
#   - The two or three changes people notice most, benefit first, in the
#     app's own words.
#   - It names Claktory once and stands on its own.
#   - No friend bug tags, ids, versions or machinery words, nothing unreleased.
#   Example: "Replay how your team grew in Claktory, let agents ask you to run
#   a command, and install Claktory on Windows."
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/lib.sh"
CHANGELOG="$ROOT/docs/CHANGELOG.md"

[[ -f "$CHANGELOG" ]] || { echo "$CHANGELOG is missing" >&2; exit 1; }

# The entries ("- …" lines) under heading $1.
entries() {
  awk -v heading="$1" '/^## /{inside = (substr($0, 4) == heading); next} inside && /^- /{n++} END{print n + 0}' "$CHANGELOG"
}

# The plain lines under heading $1 in file $2: not blank, not "- " items or
# their indented continuations, not "Released … UTC.". The summary is the
# only one a version may have.
plain_lines() {
  awk -v heading="$1" '/^## /{inside = (substr($0, 4) == heading); next}
    inside && NF && !/^- / && !/^  / && !/^Released .* UTC\.$/' "$2"
}

# The newest version heading, after Unreleased.
newest() {
  awk '/^## /{heading = substr($0, 4); if (heading != "Unreleased") {print heading; exit}}' "$CHANGELOG"
}

case "${1:-}" in
  stamp)
    kind="" summary=""
    shift
    while (($#)); do
      case "$1" in
        --incompatible) kind=incompatible; shift ;;
        --summary) summary="${2:?usage: scripts/changelog.sh stamp [--incompatible] [--summary TEXT]}"; shift 2 ;;
        *) echo "usage: scripts/changelog.sh stamp [--incompatible] [--summary TEXT]" >&2; exit 2 ;;
      esac
    done
    grep -q '^## Unreleased$' "$CHANGELOG" || { echo "docs/CHANGELOG.md has no \"## Unreleased\" heading" >&2; exit 1; }
    [[ "$summary" != *$'\n'* && "$summary" != "- "* ]] || { echo "--summary is one plain line" >&2; exit 2; }
    if (( $(plain_lines Unreleased "$CHANGELOG" | wc -l) > 1 )); then
      echo "\"## Unreleased\" has more than one plain line: only its summary may be one (the rest are \"- \" items)" >&2
      exit 1
    fi
    if [[ -n "$summary" ]]; then
      # Replaces the summary there is, or adds it as Unreleased's first line.
      awk -v summary="$summary" '
        /^## / { inside = ($0 == "## Unreleased"); print; if (inside) { print summary; put = 1 } ; next }
        inside && NF && !/^- / && !/^  / { next }
        { print }' "$CHANGELOG" >"$CHANGELOG.new"
      mv "$CHANGELOG.new" "$CHANGELOG"
    fi
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
    [[ -n "$(plain_lines "$version" "$CHANGELOG")" ]] ||
      echo "Note: $version has no summary line yet; publishing needs one (stamp --summary \"…\", or a plain line under its Released line)." >&2
    ;;
  summary)
    file="${3:-$CHANGELOG}"
    CHANGELOG="$file"
    version="${2:-$(newest)}"
    lines="$(plain_lines "$version" "$file")"
    if [[ -z "$lines" ]]; then
      echo "$version has no summary in docs/CHANGELOG.md: one plain line under its \"Released … UTC.\" line (scripts/changelog.sh says how it's written)" >&2
      exit 1
    fi
    if (( $(wc -l <<<"$lines") > 1 )); then
      echo "$version has more than one plain line in docs/CHANGELOG.md; only its summary may be one:" >&2
      echo "$lines" >&2
      exit 1
    fi
    echo "$lines"
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
    echo "usage: scripts/changelog.sh stamp [--incompatible] [--summary TEXT] | pending RELEASES_DIR | summary [VERSION [FILE]]" >&2
    exit 2
    ;;
esac
