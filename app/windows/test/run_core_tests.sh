#!/bin/sh
# Builds and runs the Windows runner's portable core tests (core_test.cpp) on
# a Mac or Linux, with the system's C++ compiler, into a temporary folder.
# On Windows, build test/CMakeLists.txt instead (README.md).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
runner="$here/../runner"
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
${CXX:-c++} -std=c++17 -Wall -Wextra -Werror -Wconversion -Wno-sign-conversion \
  -fsanitize=address,undefined -g \
  -I"$runner" \
  "$here/core_test.cpp" \
  "$runner/core/json.cpp" \
  "$runner/core/connection_store.cpp" \
  "$runner/core/window_geometry.cpp" \
  "$runner/core/launcher_plan.cpp" \
  -o "$out/core_test"
"$out/core_test"
