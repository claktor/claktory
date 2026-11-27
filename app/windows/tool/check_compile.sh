#!/bin/sh
# Compiles the runner's sources for Windows on a Mac, without building
# anything (-fsyntax-only), to catch what MSVC would refuse before a
# Windows machine builds it: clang-cl, Windows' headers from cargo-xwin's
# cache (cargo xwin has fetched them once), and Flutter's Windows headers,
# fetched once at the engine of the Flutter in PATH. clang-cl's /W4 adds
# -Wconversion here, near MSVC's C4244 and C4267 (sign changes left out, as
# MSVC's /W4 doesn't warn of them); it isn't MSVC, so the Windows build
# still decides.
#
#   app/windows/tool/check_compile.sh        all three flavors
#
# Environment: CLANG_CL (default Homebrew LLVM's), XWIN (default
# cargo-xwin's cache), FLUTTER_HEADERS (where Flutter's headers are kept,
# default ~/Library/Caches/claktory-windows/flutter-headers-<engine>).
set -eu

here=$(cd "$(dirname "$0")/.." && pwd)
clang_cl=${CLANG_CL:-/opt/homebrew/opt/llvm/bin/clang-cl}
xwin=${XWIN:-$HOME/Library/Caches/cargo-xwin/xwin}
[ -x "$clang_cl" ] || { echo "no clang-cl at $clang_cl (CLANG_CL)"; exit 2; }
[ -d "$xwin/sdk/include/um" ] || { echo "no Windows headers at $xwin (XWIN)"; exit 2; }

flutter_root=$(dirname "$(dirname "$(readlink -f "$(command -v flutter)")")")
engine=$(cat "$flutter_root/bin/internal/engine.version")
headers=${FLUTTER_HEADERS:-$HOME/Library/Caches/claktory-windows/flutter-headers-$engine}

if [ ! -f "$headers/.done" ]; then
  mkdir -p "$headers/flutter"
  tree=$(mktemp)
  curl -sf "https://api.github.com/repos/flutter/flutter/git/trees/$engine?recursive=1" -o "$tree"
  jq -r '.tree[].path' "$tree" |
    grep -E '^engine/src/flutter/shell/platform/(common/client_wrapper/include|windows/client_wrapper/include|windows/public|common/public)/.*\.h$' |
    while read -r path; do
      case $path in
        */include/flutter/*) out="$headers/flutter/$(basename "$path")" ;;
        *) out="$headers/$(basename "$path")" ;;
      esac
      curl -sf "https://raw.githubusercontent.com/flutter/flutter/$engine/$path" -o "$out"
    done
  rm -f "$tree"
  touch "$headers/.done"
fi

failed=0
for flavor in "REAL=0 DEV=0" "REAL=1 DEV=0" "REAL=0 DEV=1"; do
  real=${flavor%% *}
  dev=${flavor##* }
  for source in "$here"/runner/*.cpp "$here"/runner/core/*.cpp; do
    if ! "$clang_cl" --target=x86_64-pc-windows-msvc \
      -fms-compatibility-version=19.40 /std:c++17 /W4 /WX /wd4100 /EHsc \
      -Wconversion -Wno-sign-conversion -Wno-unused-command-line-argument \
      /D_HAS_EXCEPTIONS=0 /DUNICODE /D_UNICODE /DNOMINMAX \
      /D_WIN32_WINNT=0x0A00 /DWINVER=0x0A00 /DNTDDI_VERSION=0x0A00000B \
      "/DCLAKTORY_FLAVOR_$real" "/DCLAKTORY_FLAVOR_$dev" \
      /imsvc "$xwin/crt/include" /imsvc "$xwin/sdk/include/ucrt" \
      /imsvc "$xwin/sdk/include/um" /imsvc "$xwin/sdk/include/shared" \
      /imsvc "$xwin/sdk/include/winrt" \
      /I "$here/runner" /I "$here" /I "$headers" \
      -fsyntax-only -- "$source"; then
      echo "failed: $source ($flavor)"
      failed=1
    fi
  done
done
[ "$failed" = 0 ] && echo "all runner sources compile (three flavors)"
exit "$failed"
