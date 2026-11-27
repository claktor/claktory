#!/usr/bin/env bash
# Builds ripgrep (rg), which Claktory ships so every agent has it without
# installing anything (Matt, #215): a pinned release, built from source with
# its own Cargo.lock and its own release-lto profile, default features only
# (no pcre2; on Linux, musl, it links jemalloc, its own choice there), and the
# same Rust toolchains as Claktory's own binaries (Fugo 27535). The crate
# itself is pinned by its SHA-256, as crates.io's index had it when this was
# written: anything else stops the build.
#
#   scripts/build-rg.sh mac OUT           this Mac's (aarch64-apple-darwin);
#                                          scripts/sign-release.sh signs it
#   scripts/build-rg.sh linux ARCH OUT    static musl, x86_64 or aarch64, in
#                                          scripts/build-linux.sh's container
#   scripts/build-rg.sh licenses DIR      its license files (COPYING,
#                                          LICENSE-MIT, UNLICENSE) into DIR
#
# Only the .crate is kept, in CLAKTORY_RG_CACHE (~/Library/Caches/claktory-ripgrep,
# outside the checkout, since cargo would take ripgrep inside it for a member
# of Claktory's workspace), and its hash is checked on every build. A release's
# build unpacks it into a fresh folder and builds into a fresh target, every
# time, as every release build starts from nothing kept (scripts/lib.sh
# release_inputs).
# `--dev` (scripts/dev.sh) keeps the unpacked source and the build in the
# cache instead, for quick rebuilds; never for a release.
#
#   scripts/build-rg.sh mac OUT --dev
set -euo pipefail

RG_VERSION=15.2.0
RG_CRATE_SHA256=a30750b6d0743bfdd2656ebbaf4555aa278c43144b84bc389bcbfa399485ec71

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CACHE="${CLAKTORY_RG_CACHE:-$HOME/Library/Caches/claktory-ripgrep}"
usage() { echo "usage: scripts/build-rg.sh mac OUT [--dev] | linux x86_64|aarch64 OUT | licenses DIR" >&2; exit 2; }

DEV=false ARGS=()
for a in "$@"; do if [[ "$a" == --dev ]]; then DEV=true; else ARGS+=("$a"); fi; done
set -- "${ARGS[@]+"${ARGS[@]}"}"

# Fresh folders for a release's build, removed when it ends. In /private/tmp
# (the real path), which the container engine's VM shares.
FRESH=""
cleanup() { [[ -z "$FRESH" ]] || rm -rf "$FRESH"; }
trap cleanup EXIT
if ! $DEV; then
  source "$ROOT/scripts/lib.sh"
  FRESH="$(mktemp -d /private/tmp/claktory-rg.XXXXXX)"
  cargo_parents_ok "$FRESH"
fi

# The pinned crate, its hash checked on every build, unpacked: prints the
# source folder (fresh, or the cache's with --dev).
source_tree() {
  local crate="$CACHE/ripgrep-$RG_VERSION.crate" into
  mkdir -p "$CACHE"
  if [[ ! -f "$crate" ]]; then
    curl -fsSL -o "$crate.partial" "https://static.crates.io/crates/ripgrep/ripgrep-$RG_VERSION.crate"
    mv -f "$crate.partial" "$crate"
  fi
  local got
  got="$(shasum -a 256 "$crate" | cut -d' ' -f1)"
  if [[ "$got" != "$RG_CRATE_SHA256" ]]; then
    rm -f "$crate"
    echo "ripgrep-$RG_VERSION.crate is $got, not the pinned $RG_CRATE_SHA256: not building it" >&2
    exit 1
  fi
  if $DEV; then
    into="$CACHE/dev"
    if [[ ! -f "$into/ripgrep-$RG_VERSION/.unpacked" ]]; then
      rm -rf "$into/ripgrep-$RG_VERSION"; mkdir -p "$into"
      tar -xzf "$crate" -C "$into"
      touch "$into/ripgrep-$RG_VERSION/.unpacked"
    fi
  else
    into="$FRESH"
    tar -xzf "$crate" -C "$into"
  fi
  [[ -f "$into/ripgrep-$RG_VERSION/Cargo.lock" ]] || { echo "ripgrep-$RG_VERSION.crate has no Cargo.lock" >&2; exit 1; }
  echo "$into/ripgrep-$RG_VERSION"
}

# It's the pinned version, and it searches.
check() { # BINARY
  "$1" --version | head -1 | grep -qx "ripgrep $RG_VERSION" || { echo "$1 isn't ripgrep $RG_VERSION" >&2; exit 1; }
  printf 'claktory\n' | "$1" -q claktory || { echo "$1 doesn't search" >&2; exit 1; }
}

case "${1:-}" in
  mac)
    OUT="${2:-}"; [[ -n "$OUT" ]] || usage
    SRC="$(source_tree)"
    # This Mac's home folder stays out of it, as out of Claktory's own
    # binaries (release_rustflags): the cache and cargo's registry are in it.
    source "$ROOT/scripts/lib.sh"
    release_rustflags
    # Its own release profile, as upstream ships it (LTO, symbols stripped).
    if $DEV; then TARGET="$CACHE/dev/target-mac"; else TARGET="$FRESH/target"; fi
    # In its source folder: cargo reads the configuration of the folder it runs in.
    (cd "$SRC" && cargo build --quiet --profile release-lto --locked --manifest-path "$SRC/Cargo.toml" --target-dir "$TARGET" --bin rg)
    BIN="$TARGET/release-lto/rg"
    check "$BIN"
    if strings "$BIN" | grep -q "$HOME"; then echo "rg holds this Mac's home folder ($HOME)" >&2; exit 1; fi
    mkdir -p "$(dirname "$OUT")"
    cp "$BIN" "$OUT.partial"; mv -f "$OUT.partial" "$OUT"
    echo "built $OUT (ripgrep $RG_VERSION, aarch64-apple-darwin)"
    ;;
  linux)
    ARCH="${2:-}" OUT="${3:-}"; [[ -n "$ARCH" && -n "$OUT" ]] || usage
    SRC="$(source_tree)"
    FACTORY_LINUX_SRC="$SRC" "$ROOT/scripts/build-linux.sh" "$ARCH" "$OUT" rg
    # Checked in a container of its own architecture, natively (an x86_64
    # build on FACTORY_X86_HOST, scripts/lib.sh container_for): the pinned
    # version, and it searches.
    case "$ARCH" in x86_64) PLATFORM=linux/amd64 ;; aarch64) PLATFORM=linux/arm64 ;; *) usage ;; esac
    source "$ROOT/scripts/lib.sh"
    container_for "$ARCH"
    DIR="$(cd "$(dirname "$OUT")" && pwd -P)"
    GOT="$(engine run --rm --platform "$PLATFORM" -v "$DIR:/out:ro" docker.io/library/alpine:3 \
      sh -c "/out/$(basename "$OUT") --version | head -1; printf 'claktory\\n' | /out/$(basename "$OUT") -q claktory && echo searches" 2>/dev/null)" || true
    [[ "$GOT" == "ripgrep $RG_VERSION"$'\n'"searches" ]] || { echo "$OUT isn't a working ripgrep $RG_VERSION for $ARCH: $GOT" >&2; exit 1; }
    if strings "$OUT" | grep -q "$HOME"; then echo "$OUT holds this Mac's home folder ($HOME)" >&2; exit 1; fi
    echo "built $OUT (ripgrep $RG_VERSION, $ARCH-unknown-linux-musl, static)"
    ;;
  licenses)
    DIR="${2:-}"; [[ -n "$DIR" ]] || usage
    SRC="$(source_tree)"
    mkdir -p "$DIR"
    cp "$SRC/COPYING" "$SRC/LICENSE-MIT" "$SRC/UNLICENSE" "$DIR/"
    ;;
  *) usage ;;
esac
