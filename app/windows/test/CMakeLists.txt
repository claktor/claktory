# The runner's portable core tests on Windows (README.md):
#   cmake -S app/windows/test -B build/windows-core-test
#   cmake --build build/windows-core-test --config Debug
#   build/windows-core-test/Debug/core_test.exe
# On a Mac or Linux, run_core_tests.sh does the same.
cmake_minimum_required(VERSION 3.14)
project(claktory_core_test LANGUAGES CXX)

set(RUNNER "${CMAKE_CURRENT_SOURCE_DIR}/../runner")
add_executable(core_test
  "core_test.cpp"
  "${RUNNER}/core/connection_store.cpp"
  "${RUNNER}/core/json.cpp"
  "${RUNNER}/core/window_geometry.cpp"
  "${RUNNER}/core/launcher_plan.cpp"
)
target_compile_features(core_test PRIVATE cxx_std_17)
target_include_directories(core_test PRIVATE "${RUNNER}")
if(MSVC)
  # As strict as the app's own build (apply_standard_settings), and UTF-8
  # source for the tests' non-ASCII strings.
  target_compile_options(core_test PRIVATE /W4 /WX /wd4100 /EHsc /utf-8)
endif()
