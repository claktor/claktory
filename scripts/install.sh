#!/usr/bin/env bash
# Installs this checkout as the real Factory, for the first time or to repair
# it: scripts/release.sh builds a release, and scripts/install-release.sh
# installs it (what goes where is listed there).
#
# The backend restarts, which interrupts every agent mid-run. To ship a change
# to a Factory that's already installed, use scripts/release.sh: the app then
# offers the update. Agents: only run this when you're asked to.
#
# To install Factory on a Mac without a checkout, use the online installer
# that scripts/publish.sh uploads with each release.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RELEASES="$HOME/.factory/releases"

"$ROOT/scripts/release.sh"
"$ROOT/scripts/install-release.sh" "$RELEASES/$(cat "$RELEASES/latest")"
