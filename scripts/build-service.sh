#!/usr/bin/env bash
# What the build service (claktory-ops mac/build-user/setup.sh) runs
# for a request, as its build user (claktory-build), in an export of the
# requested commit: this file, from that commit. It builds, and only builds:
# the service's signer signs what it leaves in CLAKTORY_BUILD_OUT, and the
# person's account notarizes and publishes it (scripts/release.sh
# --from-service). Requests come from scripts/build-request.sh.
#
#   scripts/build-service.sh JOB
#
# Jobs:
#   release   the Mac release of the real Claktory (scripts/release.sh
#             --build-service), on the update channel people's Claktories
#             check, unsigned, into CLAKTORY_BUILD_OUT.
#
# The service gives it: CLAKTORY_BUILD_SERVICE=1, CLAKTORY_BUILD_SHA and
# CLAKTORY_BUILD_TREE (the commit and its tree, as git checked them),
# CLAKTORY_BUILD_OUT (an empty folder), CARGO_HOME and PUB_CACHE (empty
# folders of the job's), FLUTTER_ROOT (a copy of Flutter at
# scripts/toolchain.pins' revision) and on PATH the pinned Rust (no rustup:
# root installed it from Rust's manifest, read-only to this user) and CMake.
# A fresh home folder, removed after the job with everything this user ran.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[[ "${CLAKTORY_BUILD_SERVICE:-}" == 1 && -d "${CLAKTORY_BUILD_OUT:-}" && -n "${TMPDIR:-}" ]] || {
  echo "scripts/build-service.sh runs only inside the build service (scripts/build-request.sh asks it)" >&2
  exit 2
}
RUST_VERSION="$(sed -n 's/^RUST_VERSION=\([0-9][0-9.]*\)$/\1/p' "$ROOT/scripts/toolchain.pins")"
RUSTC="$(command -v rustc)" || { echo "no Rust on PATH" >&2; exit 1; }
RUST_BIN="$(dirname "$RUSTC")"
[[ "$("$RUSTC" -V)" == "rustc $RUST_VERSION "* ]] || { echo "the Rust on PATH isn't $RUST_VERSION" >&2; exit 1; }

# The app's tunnel is built through native_toolchain_rust, which runs
# `rustup show active-toolchain`, then `rustup run <channel> cargo build …`
# with the channel app/rust/rust-toolchain.toml pins. There's no rustup
# here: this stands in for those two, with the service's Rust, and only for
# the pinned version. Anything else fails.
SHIM="$TMPDIR/rustup-shim"
mkdir -p "$SHIM"
cat >"$SHIM/rustup" <<SHIM
#!/bin/bash
set -euo pipefail
case "\${1:-}" in
  show)
    [[ "\${2:-}" == active-toolchain ]] || { echo "rustup (build service): only 'show active-toolchain'" >&2; exit 1; }
    echo "$RUST_VERSION-aarch64-apple-darwin (the build service's)" ;;
  run)
    case "\${2:-}" in $RUST_VERSION | $RUST_VERSION-aarch64-apple-darwin) ;; *) echo "rustup (build service): only Rust $RUST_VERSION, not \${2:-}" >&2; exit 1 ;; esac
    case "\${3:-}" in cargo | rustc) ;; *) echo "rustup (build service): only cargo and rustc, not \${3:-}" >&2; exit 1 ;; esac
    tool="\$3"; shift 3
    exec "$RUST_BIN/\$tool" "\$@" ;;
  *) echo "rustup (build service): \${1:-nothing} isn't available here" >&2; exit 1 ;;
esac
SHIM
chmod 755 "$SHIM/rustup"
export PATH="$SHIM:$PATH"

case "${1:-}" in
  release)
    export FACTORY_UPDATE_URL="https://github.com/claktory/claktory/releases/latest/download/latest.json"
    exec "$ROOT/scripts/release.sh" --build-service
    ;;
  *)
    echo "scripts/build-service.sh: no job \"${1:-}\" (jobs: release)" >&2
    exit 2
    ;;
esac
