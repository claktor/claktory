#!/usr/bin/env bash
# Builds Claktory's account service (crates/claktory-account) for Linux: one
# static binary (musl), x86_64 or aarch64, in a container with podman or
# docker, the way scripts/build-linux.sh builds the backend (and sharing its
# build volumes, so later builds are quick). scripts/deploy-account.sh runs it.
#
#   scripts/build-account-linux.sh ARCH OUT    ARCH: x86_64 or aarch64; OUT: the binary to write
#
#   FACTORY_CONTAINER=docker   the container engine (podman unless it's missing)
#   FACTORY_LINUX_JOBS=N       parallel jobs (2)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ARCH="${1:-}" OUT="${2:-}"
case "$ARCH" in
  x86_64) PLATFORM=linux/amd64 ;;
  aarch64) PLATFORM=linux/arm64 ;;
  *) echo "usage: scripts/build-account-linux.sh x86_64|aarch64 OUT" >&2; exit 2 ;;
esac
[[ -n "$OUT" ]] || { echo "usage: scripts/build-account-linux.sh x86_64|aarch64 OUT" >&2; exit 2; }

ENGINE="${FACTORY_CONTAINER:-}"
if [[ -z "$ENGINE" ]]; then
  if command -v podman >/dev/null; then ENGINE=podman
  elif command -v docker >/dev/null; then ENGINE=docker
  else
    echo "building for Linux needs podman (brew install podman; podman machine init --rosetta; podman machine start) or docker" >&2
    exit 1
  fi
fi

mkdir -p "$(dirname "$OUT")"
OUT_DIR="$(cd "$(dirname "$OUT")" && pwd)"

"$ENGINE" run --rm --platform "$PLATFORM" \
  -v "$ROOT:/src:ro" -v "$OUT_DIR:/out" \
  -v "factory-linux-target-$ARCH:/target" -v "factory-linux-cargo:/usr/local/cargo/registry" \
  -e CARGO_TARGET_DIR=/target \
  docker.io/library/rust:1-alpine sh -euc '
    apk add --no-cache musl-dev >/dev/null
    mkdir /work
    cd /src
    tar --exclude=./target --exclude=./app --exclude=./.git -cf - . | tar -xf - -C /work
    cd /work
    cargo build --release --locked -p claktory-account -j "$1"
    file=/target/release/claktory-account
    if ldd "$file" 2>&1 | grep -q "=>"; then echo "claktory-account isn'"'"'t static" >&2; exit 1; fi
    "$file" help | grep -q "Usage:"
    cp "$file" "/out/.$2.partial"
  ' sh "${FACTORY_LINUX_JOBS:-2}" "$(basename "$OUT")"
mv -f "$OUT_DIR/.$(basename "$OUT").partial" "$OUT"
echo "built $OUT ($ARCH)"
