cmake_minimum_required(VERSION 3.14)
project(runner LANGUAGES CXX)

# The runner's own code that builds anywhere (core/): the tests build it on a
# Mac or Linux too (../test/README.md).
add_library(claktory_core STATIC
  "core/connection_store.cpp"
  "core/command_line.cpp"
  "core/json.cpp"
  "core/launcher_plan.cpp"
  "core/window_geometry.cpp"
)
apply_standard_settings(claktory_core)
target_include_directories(claktory_core PUBLIC "${CMAKE_CURRENT_SOURCE_DIR}")

# Define the application target. To change its name, change BINARY_NAME in the
# top-level CMakeLists.txt, not the value here, or `flutter run` will no longer
# work.
#
# Any new source files that you add to the application should be added here.
add_executable(${BINARY_NAME} WIN32
  "app_paths.cpp"
  "device_channel.cpp"
  "drop_target.cpp"
  "engine_channel.cpp"
  "encodable_json.cpp"
  "file_dialogs.cpp"
  "file_store.cpp"
  "flutter_window.cpp"
  "main.cpp"
  "network_channel.cpp"
  "notifications_channel.cpp"
  "presence.cpp"
  "setup_channel.cpp"
  "sight_channel.cpp"
  "toast_activator.cpp"
  "utils.cpp"
  "win32_window.cpp"
  "window_channel.cpp"
  "window_placement.cpp"
  "${FLUTTER_MANAGED_DIR}/generated_plugin_registrant.cc"
  "Runner.rc"
  "runner.exe.manifest"
)

# Apply the standard set of build settings. This can be removed for applications
# that need different build settings.
apply_standard_settings(${BINARY_NAME})

# Add preprocessor definitions for the build version.
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION=\"${FLUTTER_VERSION}\"")
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION_MAJOR=${FLUTTER_VERSION_MAJOR}")
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION_MINOR=${FLUTTER_VERSION_MINOR}")
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION_PATCH=${FLUTTER_VERSION_PATCH}")
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION_BUILD=${FLUTTER_VERSION_BUILD}")

# Which Claktory this is (flavor.h; the top-level CMakeLists.txt says how it's
# chosen). The real one only in a Release build.
target_compile_definitions(${BINARY_NAME} PRIVATE
  "CLAKTORY_FLAVOR_REAL=$<AND:$<CONFIG:Release>,$<BOOL:${CLAKTORY_WANTS_REAL}>>"
  "CLAKTORY_FLAVOR_DEV=$<BOOL:${CLAKTORY_WANTS_DEV}>"
)

# Disable Windows macros that collide with C++ standard library functions.
target_compile_definitions(${BINARY_NAME} PRIVATE "NOMINMAX")
# Windows 11's API (NTDDI_WIN10_CO) as the floor the headers offer: the app
# supports Windows 11 (windows.md section 6), and calls what it has directly
# (the network's connectivity hints, Windows 10 2004 and later, among them).
target_compile_definitions(${BINARY_NAME} PRIVATE
  "_WIN32_WINNT=0x0A00" "WINVER=0x0A00" "NTDDI_VERSION=0x0A00000B")

# Add dependency libraries and include directories. Add any application-specific
# dependencies here.
target_link_libraries(${BINARY_NAME} PRIVATE flutter flutter_wrapper_app claktory_core)
target_link_libraries(${BINARY_NAME} PRIVATE
  "dwmapi.lib" "crypt32.lib" "shell32.lib" "ole32.lib" "advapi32.lib" "uuid.lib" "iphlpapi.lib"
  "runtimeobject.lib" "wtsapi32.lib" "propsys.lib")
target_include_directories(${BINARY_NAME} PRIVATE "${CMAKE_SOURCE_DIR}")

# Run the Flutter tool portions of the build. This must not be removed.
add_dependencies(${BINARY_NAME} flutter_assemble)

# The smoke test (../test/windows_smoke.cpp: the private folder, DPAPI and the
# connection store on disk, each part in a child process of its own). Not part
# of the app; built on its own:
#   cmake --build build\windows\x64 --config Debug --target windows_smoke
add_executable(windows_smoke EXCLUDE_FROM_ALL
  "../test/windows_smoke.cpp"
  "app_paths.cpp"
  "file_store.cpp"
  "utils.cpp"
)
apply_standard_settings(windows_smoke)
target_compile_definitions(windows_smoke PRIVATE
  "CLAKTORY_FLAVOR_REAL=0" "CLAKTORY_FLAVOR_DEV=0" "NOMINMAX"
  "_WIN32_WINNT=0x0A00" "WINVER=0x0A00" "NTDDI_VERSION=0x0A00000B")
target_include_directories(windows_smoke PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}" "${CMAKE_SOURCE_DIR}")
target_link_libraries(windows_smoke PRIVATE flutter claktory_core "crypt32.lib")
