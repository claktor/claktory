#!/usr/bin/env bash
# Builds jq, which Claktory ships beside ripgrep so every agent can read and
# filter JSON without installing anything (Bruno 29754): a pinned release,
# built from its source tarball with its bundled Oniguruma (regular
# expressions), no documentation, and static on Linux (musl). The tarball is
# the one jq's own CI built from its jq-1.8.2 tag and attested
# (`gh attestation verify jq-1.8.2.tar.gz --bundle jq-attestation.json
# --repo jqlang/jq`), pinned by its SHA-256: anything else stops the build.
# 1.8.2 is the release with the 2026 security fixes; never an older one.
#
#   scripts/build-jq.sh mac OUT           this Mac's (aarch64-apple-darwin);
#                                          scripts/sign-release.sh signs it
#   scripts/build-jq.sh linux ARCH OUT    static musl, x86_64 or aarch64, in
#                                          scripts/build-linux.sh's image
#   scripts/build-jq.sh licenses DIR      its license files (COPYING, which
#                                          has the notices of the code it
#                                          includes, and Oniguruma's) into DIR
#
# Each build runs jq's own test suite (and Oniguruma's) when it can run what
# it built: on the Mac, and on Linux for the container's own architecture.
#
# Only the tarball is kept, in CLAKTORY_JQ_CACHE (~/Library/Caches/claktory-jq),
# and its hash is checked on every build; every build unpacks it into a fresh
# folder. `--dev` (scripts/dev.sh) is accepted for the same calls as
# scripts/build-rg.sh and builds the same way.
set -euo pipefail

JQ_VERSION=1.8.2
JQ_TARBALL_SHA256=71b8d6e8f5fe81f6c6d0d110e3892251f6ce76ed095abd315e26e6e1193af3af

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CACHE="${CLAKTORY_JQ_CACHE:-$HOME/Library/Caches/claktory-jq}"
usage() { echo "usage: scripts/build-jq.sh mac OUT [--dev] | linux x86_64|aarch64 OUT | licenses DIR" >&2; exit 2; }

ARGS=()
for a in "$@"; do [[ "$a" == --dev ]] || ARGS+=("$a"); done
set -- "${ARGS[@]+"${ARGS[@]}"}"

# jq's own configuration: its bundled Oniguruma, one static program, no
# documentation (it would need its own build tools) and nothing of the
# maintainer's (it would regenerate the parser).
CONFIGURE=(--disable-docs --disable-shared --enable-static --with-oniguruma=builtin --disable-maintainer-mode --disable-valgrind)

# A fresh folder, removed when it ends. In /private/tmp (the real path),
# which the container engine's VM shares.
FRESH="$(mktemp -d /private/tmp/claktory-jq.XXXXXX)"
trap 'rm -rf "$FRESH"' EXIT

# The pinned tarball, its hash checked on every build.
tarball() {
  local file="$CACHE/jq-$JQ_VERSION.tar.gz"
  mkdir -p "$CACHE"
  if [[ ! -f "$file" ]]; then
    curl -fsSL -o "$file.partial" "https://github.com/jqlang/jq/releases/download/jq-$JQ_VERSION/jq-$JQ_VERSION.tar.gz"
    mv -f "$file.partial" "$file"
  fi
  local got
  got="$(shasum -a 256 "$file" | cut -d' ' -f1)"
  if [[ "$got" != "$JQ_TARBALL_SHA256" ]]; then
    rm -f "$file"
    echo "jq-$JQ_VERSION.tar.gz is $got, not the pinned $JQ_TARBALL_SHA256: not building it" >&2
    exit 1
  fi
  echo "$file"
}

# Unpacked into the fresh folder: prints the source folder.
source_tree() {
  tar -xzf "$(tarball)" -C "$FRESH"
  [[ -f "$FRESH/jq-$JQ_VERSION/configure" && -f "$FRESH/jq-$JQ_VERSION/vendor/oniguruma/configure" ]] ||
    { echo "jq-$JQ_VERSION.tar.gz has no configure or no bundled Oniguruma" >&2; exit 1; }
  echo "$FRESH/jq-$JQ_VERSION"
}

# It's the pinned version, and it reads JSON.
check() { # BINARY
  [[ "$("$1" --version)" == "jq-$JQ_VERSION" ]] || { echo "$1 isn't jq $JQ_VERSION" >&2; exit 1; }
  [[ "$(printf '{"a":[1,{"b":"claktory"}]}' | "$1" -r '.a[1].b')" == claktory ]] || { echo "$1 doesn't read JSON" >&2; exit 1; }
}

case "${1:-}" in
  mac)
    OUT="${2:-}"; [[ -n "$OUT" ]] || usage
    SRC="$(source_tree)"
    (
      cd "$SRC"
      CFLAGS="-O2" ./configure "${CONFIGURE[@]}" >"$FRESH/configure.log" 2>&1 || { tail -30 "$FRESH/configure.log" >&2; exit 1; }
      make -j "$(sysctl -n hw.ncpu)" >"$FRESH/make.log" 2>&1 || { tail -30 "$FRESH/make.log" >&2; exit 1; }
      make -j "$(sysctl -n hw.ncpu)" check >"$FRESH/check.log" 2>&1 || { grep -E '^(FAIL|ERROR):' "$FRESH/check.log" >&2; tail -30 "$FRESH/check.log" >&2; exit 1; }
    )
    BIN="$SRC/jq"
    strip "$BIN"
    check "$BIN"
    # The system's libraries only.
    if otool -L "$BIN" | tail -n +2 | grep -v '^[[:space:]]*/usr/lib/'; then echo "jq links more than the system's libraries" >&2; exit 1; fi
    if strings "$BIN" | grep -q "$HOME"; then echo "jq holds this Mac's home folder ($HOME)" >&2; exit 1; fi
    mkdir -p "$(dirname "$OUT")"
    cp "$BIN" "$OUT.partial"; mv -f "$OUT.partial" "$OUT"
    echo "built $OUT (jq $JQ_VERSION, aarch64-apple-darwin)"
    ;;
  linux)
    ARCH="${2:-}" OUT="${3:-}"; [[ -n "$ARCH" && -n "$OUT" ]] || usage
    case "$ARCH" in x86_64) PLATFORM=linux/amd64 ;; aarch64) PLATFORM=linux/arm64 ;; *) usage ;; esac
    TARBALL="$(tarball)"
    cp "$TARBALL" "$FRESH/"
    ENGINE="${FACTORY_CONTAINER:-$(command -v podman >/dev/null && echo podman || echo docker)}"
    # scripts/build-linux.sh's image, by its digest (one pin for every Linux
    # build). Pulled by that digest when this machine doesn't have it; a
    # release's run pulled it again already (FACTORY_LINUX_FRESH).
    IMAGE="$(sed -n 's/^IMAGE="\(docker\.io\/library\/rust@sha256:[0-9a-f]\{64\}\)"$/\1/p' "$ROOT/scripts/build-linux.sh")"
    [[ -n "$IMAGE" ]] || { echo "no pinned image in scripts/build-linux.sh" >&2; exit 1; }
    # The container runs natively and cross-compiles with zig, as
    # scripts/build-linux.sh does (emulation takes far longer).
    case "$(uname -m)" in arm64 | aarch64) NATIVE=linux/arm64 NATIVE_ARCH=aarch64 ;; *) NATIVE=linux/amd64 NATIVE_ARCH=x86_64 ;; esac
    "$ENGINE" image inspect "$IMAGE" >/dev/null 2>&1 || "$ENGINE" pull -q --platform "$NATIVE" "$IMAGE" >/dev/null
    mkdir -p "$(dirname "$OUT")"
    OUT_DIR="$(cd "$(dirname "$OUT")" && pwd -P)"
    rm -f "$OUT_DIR/.$(basename "$OUT").partial"
    if ! "$ENGINE" run --rm --platform "$NATIVE" \
      -v "$FRESH:/in:ro" -v "$OUT_DIR:/out" \
      "$IMAGE" sh -euc '
        VERSION=$1 ARCH=$2 NATIVE_ARCH=$3 NAME=$4
        shift 4
        # tzdata only for its tests (local time in Asia/Tokyo): jq reads the
        # time zones of the machine it runs on.
        apk add --no-cache musl-dev zig make file tzdata >/dev/null
        mkdir /work && cd /work
        tar -xzf "/in/jq-$VERSION.tar.gz"
        cd "jq-$VERSION"
        CC="zig cc -target $ARCH-linux-musl" CFLAGS="-O2" LDFLAGS="-s" \
          ./configure --host="$ARCH-linux-musl" --enable-all-static "$@" >/tmp/configure.log 2>&1 || { tail -30 /tmp/configure.log >&2; exit 1; }
        make -j 4 >/tmp/make.log 2>&1 || { tail -30 /tmp/make.log >&2; exit 1; }
        # Static, or it would need the libraries of the distribution it was built on.
        if ! file jq | grep -Eq "statically linked|static-pie linked"; then file jq >&2; echo "jq isn'"'"'t static" >&2; exit 1; fi
        # Its test suites, when it can run here.
        if [ "$ARCH" = "$NATIVE_ARCH" ]; then
          make -j 4 check >/tmp/check.log 2>&1 || { grep -E "^(FAIL|ERROR):" /tmp/check.log >&2; tail -30 /tmp/check.log >&2; exit 1; }
          echo "jq test suites passed ($ARCH)"
        fi
        cp jq "/out/.$NAME.partial"
      ' sh "$JQ_VERSION" "$ARCH" "$NATIVE_ARCH" "$(basename "$OUT")" "${CONFIGURE[@]}"; then
      echo "jq's Linux build ($ARCH) failed" >&2; exit 1
    fi
    mv -f "$OUT_DIR/.$(basename "$OUT").partial" "$OUT"
    # Checked in a container of its own architecture (emulated when it isn't
    # this machine's): the pinned version, and it reads JSON.
    GOT="$("$ENGINE" run --rm --platform "$PLATFORM" -v "$OUT_DIR:/out:ro" docker.io/library/alpine:3 \
      sh -c "/out/$(basename "$OUT") --version; printf '{\"a\":[1,{\"b\":\"claktory\"}]}' | /out/$(basename "$OUT") -r '.a[1].b'" 2>/dev/null)" || true
    [[ "$GOT" == "jq-$JQ_VERSION"$'\n'"claktory" ]] || { echo "$OUT isn't a working jq $JQ_VERSION for $ARCH: $GOT" >&2; rm -f "$OUT"; exit 1; }
    if strings "$OUT" | grep -q "$HOME"; then echo "$OUT holds this Mac's home folder ($HOME)" >&2; rm -f "$OUT"; exit 1; fi
    echo "built $OUT (jq $JQ_VERSION, $ARCH-linux-musl, static)"
    ;;
  licenses)
    DIR="${2:-}"; [[ -n "$DIR" ]] || usage
    SRC="$(source_tree)"
    mkdir -p "$DIR"
    cp "$SRC/COPYING" "$DIR/COPYING"
    cp "$SRC/vendor/oniguruma/COPYING" "$DIR/COPYING.oniguruma"
    ;;
  *) usage ;;
esac
