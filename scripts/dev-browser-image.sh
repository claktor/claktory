#!/usr/bin/env bash
# Builds this checkout's browser image (its Linux runner, from these sources)
# into podman for the dev Claktory, and prints its id. The dev backend runs
# it through podman, rather than Apple's container tool, when started with
# that id (an image's id is its content's hash, so it can't move to another):
#
#   FACTORY_BROWSER_PODMAN="$(scripts/dev-browser-image.sh)" scripts/dev.sh backend
#
# For a dev Claktory started from an agent's shell: on a Mac where Claktory's
# browser is set up, agents' commands can't reach Apple's container tool, and
# a dev backend started from one is under the same rule. Only the dev
# Claktory (~/.factory-dev) takes FACTORY_BROWSER_PODMAN. Rebuild after
# changing the runner (crates/factoryd/src/browser_runner, errands' wire),
# the image (scripts/browser-image/) or Chrome's policy.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib.sh
source "$ROOT/scripts/lib.sh"
command -v podman >/dev/null || { echo "dev-browser-image.sh: podman is needed (brew install podman)" >&2; exit 1; }
version="$(stamp_version)"
out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT
FACTORY_LINUX_SRC="$ROOT" "$ROOT/scripts/build-linux.sh" aarch64 "$out/runner" >&2
ENGINE=podman CONTAINER=none "$ROOT/scripts/browser-image.sh" archive "$version" "$out/runner" "$out" --dev >&2
podman image inspect --format '{{.Id}}' "localhost/claktory-browser-dev:$version"
