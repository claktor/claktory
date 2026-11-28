# The Start menu's Claktory (main.cpp; windows-updates.md section 5): the
# small program at %LOCALAPPDATA%\Programs\Claktory\Claktory.exe that starts
# the version app\current names. Built with the app and installed in its
# bundle as launcher\Claktory.exe, for the installer and the release to put
# beside app\, where it runs with no app DLLs or runtime next to it: the C++
# runtime is linked in.
cmake_minimum_required(VERSION 3.14)
project(launcher LANGUAGES CXX)

# The static C++ runtime, in this folder only (its flags are this
# directory's): nothing beside the launcher to load.
foreach(config DEBUG RELEASE PROFILE)
  foreach(language C CXX)
    string(REPLACE "/MDd" "/MTd" CMAKE_${language}_FLAGS_${config}
      "${CMAKE_${language}_FLAGS_${config}}")
    string(REPLACE "/MD" "/MT" CMAKE_${language}_FLAGS_${config}
      "${CMAKE_${language}_FLAGS_${config}}")
  endforeach()
endforeach()

add_executable(claktory_launcher WIN32
  "main.cpp"
  "../runner/core/launcher_plan.cpp"
  "launcher.rc"
  "launcher.exe.manifest"
)
apply_standard_settings(claktory_launcher)
set_target_properties(claktory_launcher PROPERTIES OUTPUT_NAME "Claktory")
# The same Claktory as the app beside it (flavor.h): its name and icon.
target_compile_definitions(claktory_launcher PRIVATE
  "CLAKTORY_FLAVOR_REAL=$<AND:$<CONFIG:Release>,$<BOOL:${CLAKTORY_WANTS_REAL}>>"
  "CLAKTORY_FLAVOR_DEV=$<BOOL:${CLAKTORY_WANTS_DEV}>"
  "NOMINMAX" "UNICODE" "_UNICODE"
  "_WIN32_WINNT=0x0A00" "WINVER=0x0A00" "NTDDI_VERSION=0x0A00000B")
# The app's version (--build-name and --build-number), as the runner's.
target_compile_definitions(claktory_launcher PRIVATE
  "FLUTTER_VERSION=\"${FLUTTER_VERSION}\""
  "FLUTTER_VERSION_MAJOR=${FLUTTER_VERSION_MAJOR}"
  "FLUTTER_VERSION_MINOR=${FLUTTER_VERSION_MINOR}"
  "FLUTTER_VERSION_PATCH=${FLUTTER_VERSION_PATCH}"
  "FLUTTER_VERSION_BUILD=${FLUTTER_VERSION_BUILD}")
target_include_directories(claktory_launcher PRIVATE
  "${CMAKE_CURRENT_SOURCE_DIR}/../runner")
target_link_libraries(claktory_launcher PRIVATE "user32.lib")
# Installed by ../CMakeLists.txt, once the bundle's folder is known.
