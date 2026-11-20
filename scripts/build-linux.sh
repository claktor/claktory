#!/usr/bin/env bash
# Builds Factory's backend (or another of its binaries: the account service,
# claktory-account) for Linux: one static binary (musl), which runs on
# any distribution, x86_64 or aarch64. It builds in a container, with podman or
# docker, so this Mac needs no Linux toolchain. The container is this machine's
# own architecture and cross-compiles for the other with zig (cargo-zigbuild),
# since building under emulation takes hours. FACTORY_VERSION and
# FACTORY_UPDATE_URL are stamped in as for the Mac build. scripts/publish.sh
# runs it for each release.
#
#   scripts/build-linux.sh ARCH OUT [PACKAGE]
#       ARCH: x86_64 or aarch64; OUT: the binary to write; PACKAGE: factoryd
#       (the default), claktory-apps (the apps host) or claktory-account
#
#   FACTORY_CONTAINER=docker   the container engine (podman unless it's missing)
#   FACTORY_LINUX_JOBS=N       parallel jobs (2: more can run a small VM out of memory)
#
# Each arch keeps its build in a volume, factory-linux-target-ARCH, and the
# crates it downloads in factory-linux-cargo, so later builds are quick.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ARCH="${1:-}" OUT="${2:-}" PACKAGE="${3:-factoryd}"
usage() { echo "usage: scripts/build-linux.sh x86_64|aarch64 OUT [factoryd|claktory-apps|claktory-account]" >&2; exit 2; }
case "$ARCH" in
  x86_64 | aarch64) ;;
  *) usage ;;
esac
[[ -n "$OUT" ]] || usage
case "$PACKAGE" in
  factoryd | claktory-apps | claktory-account) ;;
  *) usage ;;
esac

ENGINE="${FACTORY_CONTAINER:-}"
if [[ -z "$ENGINE" ]]; then
  if command -v podman >/dev/null; then ENGINE=podman
  elif command -v docker >/dev/null; then ENGINE=docker
  else
    echo "building for Linux needs podman (brew install podman; podman machine init; podman machine start) or docker" >&2
    exit 1
  fi
fi

# The container runs natively: the same architecture as this machine.
case "$(uname -m)" in
  arm64 | aarch64) PLATFORM=linux/arm64 ;;
  *) PLATFORM=linux/amd64 ;;
esac
TARGET="$ARCH-unknown-linux-musl"

mkdir -p "$(dirname "$OUT")"
# The real path: the container engine's VM shares /private, not the /tmp
# symlink to it.
OUT_DIR="$(cd "$(dirname "$OUT")" && pwd -P)"
IMAGE="docker.io/library/rust:1-alpine"

# The checkout goes in read-only and is copied, so nothing in it changes; the
# copy leaves out the Mac's build output and the app.
"$ENGINE" run --rm --platform "$PLATFORM" \
  -v "$ROOT:/src:ro" -v "$OUT_DIR:/out" \
  -v "factory-linux-target-$ARCH:/target" -v "factory-linux-cargo:/usr/local/cargo/registry" \
  -e FACTORY_VERSION -e FACTORY_UPDATE_URL -e CARGO_TARGET_DIR=/target \
  "$IMAGE" sh -euc '
    apk add --no-cache musl-dev zig cargo-zigbuild file >/dev/null
    rustup target add "$3" >/dev/null 2>&1
    mkdir /work
    cd /src
    # Fresh modification times (-m): cargo tells what changed by them, and a
    # checkout whose files are older than what another checkout built into
    # the same volume would otherwise get those stale crates linked in.
    tar --exclude=./target --exclude=./app --exclude=./.git -cf - . | tar -xmf - -C /work
    cd /work
    cargo zigbuild --release --locked -p "$4" --target "$3" -j "$1"
    file="/target/$3/release/$4"
    # Static, or it would need the libraries of the distribution it was built on.
    if ! file "$file" | grep -Eq "statically linked|static-pie linked"; then echo "$4 isn'"'"'t static" >&2; exit 1; fi
    # And it runs, when it can here: it answers with its usage.
    if [ "$3" = "$(uname -m)-unknown-linux-musl" ]; then
      case "$4" in
        factoryd) "$file" not-a-command 2>&1 | grep -q usage ;;
        # Frames on stdin: with none, it stops cleanly.
        claktory-apps) "$file" </dev/null >/dev/null 2>&1 ;;
        *) "$file" help 2>&1 | grep -q Usage: ;;
      esac
    fi
    cp "$file" "/out/.$2.partial"
  ' sh "${FACTORY_LINUX_JOBS:-2}" "$(basename "$OUT")" "$TARGET" "$PACKAGE"
mv -f "$OUT_DIR/.$(basename "$OUT").partial" "$OUT"
echo "built $OUT ($ARCH)"
