#!/usr/bin/env bash
# Checks that no secret is anywhere in a commit's history, before it goes
# public (scripts/publish.sh runs it and stops on any finding). It scans every
# change of every commit REV reaches, deleted files included, every commit
# message, and REV's own files as they are, with gitleaks: its own rules (API keys, tokens, private keys of
# every kind) plus .gitleaks.toml's (credential and database files, paths in
# a real home folder), plus this machine's own names (your user name, home
# folder and host name). The config is read from REV itself, so the exceptions
# that apply are the committed, reviewed ones; findings are printed redacted,
# with the rule, file, line and commit.
#
#   scripts/check-secrets.sh [REV]     REV defaults to HEAD
#
# Needs gitleaks 8.25 or later: brew install gitleaks.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REV="${1:-HEAD}"

command -v gitleaks >/dev/null || {
  echo "the secret check needs gitleaks: brew install gitleaks" >&2
  exit 1
}
version="$(gitleaks version | sed 's/^v//')"
IFS=. read -r major minor _ <<<"$version"
if ((major < 8 || major == 8 && minor < 25)); then
  echo "the secret check needs gitleaks 8.25 or later, not $version: brew upgrade gitleaks" >&2
  exit 1
fi

SHA="$(git -C "$ROOT" rev-parse --verify "$REV^{commit}")"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
git -C "$ROOT" show "$SHA:.gitleaks.toml" >"$WORK/factory.toml" 2>/dev/null || {
  echo "$REV has no .gitleaks.toml; the secret check needs it" >&2
  exit 1
}

# This machine's names, which a log or path pasted into a file would carry.
literal() { printf '%s' "$1" | sed 's/[][\.*^$?+(){}|/]/\\&/g'; }
names=("$(literal "$HOME")")
user="$(id -un)"
[[ ${#user} -ge 3 && "$user" != "user" ]] && names+=("$(literal "$user")")
for host in "$(hostname -s)" "$(scutil --get LocalHostName 2>/dev/null || true)"; do
  [[ ${#host} -ge 3 && "$host" != "localhost" ]] && names+=("$(literal "$host")")
done
pattern="$(IFS='|'; echo "${names[*]}")"
cat >"$WORK/config.toml" <<TOML
[extend]
path = "$WORK/factory.toml"

[[rules]]
id = "this-machine"
description = "This machine's user name, home folder or host name"
regex = '''(?i)(?:$pattern)'''
TOML

# One line per finding: what, where and in which commit, never the secret itself.
cat >"$WORK/finding.tmpl" <<'TEMPLATE'
{{ range . }}  {{ .RuleID }} ({{ .Description }}): {{ if .Commit }}{{ .File }}:{{ .StartLine }} in commit {{ printf "%.10s" .Commit }}{{ else if .File }}{{ .File }}:{{ .StartLine }}, as it is now{{ else }}a commit message, line {{ .StartLine }}{{ end }}
{{ end }}
TEMPLATE
scan() { gitleaks "$@" --no-banner --log-level error --redact --config "$WORK/config.toml" --report-format template --report-template "$WORK/finding.tmpl"; }

count="$(git -C "$ROOT" rev-list --count "$SHA")"
status=0
# -m: a merge's changes against each parent too, so nothing a merge brings is skipped.
scan git --log-opts="-m --full-history $SHA" --report-path "$WORK/history.txt" "$ROOT" || status=1
git -C "$ROOT" log -m --format='commit %H%n%B' "$SHA" | scan stdin --report-path "$WORK/messages.txt" || status=1
# Its files as they are, outside git: what anyone downloading it gets.
mkdir "$WORK/tree"
git -C "$ROOT" archive "$SHA" | tar -x -C "$WORK/tree"
# From inside it, so files are named as in the repository (crates/…), as in
# the history scan: .gitleaks.toml's path exceptions are anchored there.
(cd "$WORK/tree" && scan dir --report-path "$WORK/tree.txt" .) || status=1

if ((status)); then
  echo "SECRET CHECK FAILED for $REV ($SHA): don't publish. Found:" >&2
  # A message's line counts from the top of `git log -m --format='commit %H%n%B' $SHA`.
  cat "$WORK/history.txt" "$WORK/messages.txt" >&2
  sed "s|$WORK/tree/||" "$WORK/tree.txt" >&2
  echo "Remove what's real from the history before publishing. A false positive is allowed only by a" >&2
  echo "narrow, explained exception in .gitleaks.toml, committed so it's reviewed." >&2
  exit 1
fi
echo "secret check passed: $count commits of $REV ($SHA), their changes and messages, and its files"
