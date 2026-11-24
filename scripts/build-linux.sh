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
#       (the default), claktory-apps (the apps host), claktory-account,
#       claktory-speech, claktory-qa, claktory-push (the push relay) or
#       claktory-user-apps (the account service's apps worker); or rg
#       (ripgrep, from the source tree scripts/build-rg.sh gives as
#       FACTORY_LINUX_SRC: its package ripgrep, its binary rg)
#
#   FACTORY_LINUX_SRC=DIR      build this source tree instead of the checkout
#       (scripts/qa.sh --container builds a commit's `git archive` this way)
#
#   FACTORY_CONTAINER=docker   the container engine (podman unless it's missing)
#   FACTORY_LINUX_JOBS=N       parallel jobs (2: more can run a small VM out of memory)
#
# Each arch keeps its build in a volume, factory-linux-target-ARCH, and the
# crates it downloads in factory-linux-cargo, so later builds are quick.
#
#   FACTORY_LINUX_FRESH=1      keep nothing from earlier builds (a release,
#       scripts/lib.sh release_inputs): the image is pulled again by its
#       digest, and the build and crates go in volumes of this run's own,
#       factory-linux-run-RUN-target-ARCH and factory-linux-run-RUN-cargo,
#       which start empty (every crate downloaded again and checked against
#       Cargo.lock) and are shared by the run's builds; RUN is
#       FACTORY_LINUX_RUN (scripts/publish.sh's, which removes them after),
#       or none at all without it.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ARCH="${1:-}" OUT="${2:-}" PACKAGE="${3:-factoryd}"
usage() { echo "usage: scripts/build-linux.sh x86_64|aarch64 OUT [factoryd|claktory-apps|claktory-account|claktory-speech|claktory-qa|claktory-push|claktory-user-apps|rg]" >&2; exit 2; }
case "$ARCH" in
  x86_64 | aarch64) ;;
  *) usage ;;
esac
[[ -n "$OUT" ]] || usage
case "$PACKAGE" in
  factoryd | claktory-apps | claktory-account | claktory-speech | claktory-qa | claktory-push | claktory-user-apps) ;;
  rg) [[ -n "${FACTORY_LINUX_SRC:-}" ]] || { echo "rg builds from ripgrep's source: run scripts/build-rg.sh linux ARCH OUT" >&2; exit 2; } ;;
  *) usage ;;
esac
SRC_ROOT="${FACTORY_LINUX_SRC:-$ROOT}"
FRESH=false; [[ "${FACTORY_LINUX_FRESH:-}" != 1 ]] || FRESH=true
# ripgrep, and a fresh build, build into the container's own /tmp, gone when
# it ends; the rest into the build volume.
RUN="${FACTORY_LINUX_RUN:-}"
TARGET_DIR=/target; { [[ "$PACKAGE" != rg ]] && { ! $FRESH || [[ -n "$RUN" ]]; }; } || TARGET_DIR=/tmp/target

# claktory-speech builds ggml (C++, cmake) into itself. A release must not
# take the build machine's CPU as the floor: every x86_64 since Haswell
# (2013) has AVX2, FMA and F16C, which is what claktory-speech check asks
# for; plain NEON is every aarch64.
SPEECH_CMAKE_ARGS=""
if [[ "$PACKAGE" == claktory-speech ]]; then
  case "$ARCH" in
    x86_64) SPEECH_CMAKE_ARGS="-DGGML_NATIVE=OFF -DGGML_AVX2=ON -DGGML_FMA=ON -DGGML_F16C=ON" ;;
    aarch64) SPEECH_CMAKE_ARGS="-DGGML_NATIVE=OFF" ;;
  esac
fi

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
# Rust 1.97.1, as the Mac builds with (scripts/check-toolchain.sh), by the
# digest of its image index on Docker Hub.
IMAGE="docker.io/library/rust@sha256:3c38f3f82c2f3d73da3b38e18d279393a04cb43ddded0e35088a8c3324d40900"
VOLUMES=(-v "factory-linux-target-$ARCH:/target" -v "factory-linux-cargo:/usr/local/cargo/registry")
if $FRESH; then
  VOLUMES=()
  [[ -z "$RUN" ]] || VOLUMES=(-v "factory-linux-run-$RUN-target-$ARCH:/target" -v "factory-linux-run-$RUN-cargo:/usr/local/cargo/registry")
fi
# Once per run: a run's first build makes its crates volume.
if $FRESH && { [[ -z "$RUN" ]] || ! "$ENGINE" volume inspect "factory-linux-run-$RUN-cargo" >/dev/null 2>&1; }; then
  # Removed first (refused while another build uses it: try again then), so
  # what's pulled is all Docker Hub's, none of it kept on this machine.
  if ID="$("$ENGINE" image inspect --format '{{.Id}}' "$IMAGE" 2>/dev/null)" && ! "$ENGINE" rmi "$ID" >/dev/null; then
    echo "another build is using the Linux build image; try again when it's done" >&2
    exit 1
  fi
  "$ENGINE" pull -q --platform "$PLATFORM" "$IMAGE" >/dev/null
fi

# The checkout goes in read-only and is copied, so nothing in it changes; the
# copy leaves out the Mac's build output and the app.
"$ENGINE" run --rm --platform "$PLATFORM" \
  -v "$SRC_ROOT:/src:ro" -v "$OUT_DIR:/out" \
  ${VOLUMES[@]+"${VOLUMES[@]}"} \
  -e FACTORY_VERSION -e FACTORY_UPDATE_URL -e CARGO_TARGET_DIR="$TARGET_DIR" \
  -e TRANSCRIBE_CMAKE_ARGS="$SPEECH_CMAKE_ARGS" \
  "$IMAGE" sh -euc '
    apk add --no-cache musl-dev zig cargo-zigbuild file >/dev/null
    # claktory-speech builds its C++ engine with cmake.
    [ "$4" != claktory-speech ] || apk add --no-cache cmake make >/dev/null
    # ripgrep on musl links jemalloc (its own choice of allocator there), built with make.
    [ "$4" != rg ] || apk add --no-cache make >/dev/null
    rustup target add "$3" >/dev/null 2>&1
    mkdir /work
    cd /src
    # Fresh modification times (-m): cargo tells what changed by them, and a
    # checkout whose files are older than what another checkout built into
    # the same volume would otherwise get those stale crates linked in.
    tar --exclude=./target --exclude=./app --exclude=./.git -cf - . | tar -xmf - -C /work
    cd /work
    if [ "$4" = rg ]; then
      # Its own release profile, as upstream ships it (LTO, symbols stripped).
      cargo zigbuild --profile release-lto --locked -p ripgrep --bin rg --target "$3" -j "$1"
    else
      cargo zigbuild --release --locked -p "$4" --target "$3" -j "$1"
    fi
    profile=release; [ "$4" != rg ] || profile=release-lto
    file="$CARGO_TARGET_DIR/$3/$profile/$4"
    # Static, or it would need the libraries of the distribution it was built on.
    if ! file "$file" | grep -Eq "statically linked|static-pie linked"; then echo "$4 isn'"'"'t static" >&2; exit 1; fi
    # And it runs, when it can here: it answers with its usage.
    if [ "$3" = "$(uname -m)-unknown-linux-musl" ]; then
      case "$4" in
        factoryd) "$file" not-a-command 2>&1 | grep -q usage ;;
        # Frames on stdin: with none, it stops cleanly.
        claktory-apps) "$file" </dev/null >/dev/null 2>&1 ;;
        claktory-speech) "$file" help 2>&1 | grep -q usage && "$file" check | grep -q supported ;;
        claktory-qa) "$file" help 2>&1 | grep -q claktory-qa ;;
        claktory-user-apps) "$file" help 2>&1 | grep -q "the worker for" ;;
        rg) "$file" --version | grep -q "^ripgrep " && printf "claktory\\n" | "$file" -q claktory ;;
        *) "$file" help 2>&1 | grep -q Usage: ;;
      esac
    fi
    cp "$file" "/out/.$2.partial"
  ' sh "${FACTORY_LINUX_JOBS:-2}" "$(basename "$OUT")" "$TARGET" "$PACKAGE"
mv -f "$OUT_DIR/.$(basename "$OUT").partial" "$OUT"
echo "built $OUT ($ARCH)"
