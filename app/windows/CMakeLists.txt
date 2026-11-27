# Project-level configuration.
cmake_minimum_required(VERSION 3.14)
project(factory_app LANGUAGES CXX)

# The name of the executable created for the application. Change this to change
# the on-disk name of your application. The Windows app is new, so it carries
# the product's name; the Dart package keeps its old one (AGENTS.md).
set(BINARY_NAME "Claktory")

# Explicitly opt in to modern CMake behaviors to avoid warnings with recent
# versions of CMake.
cmake_policy(VERSION 3.14...3.25)

# Define build configuration option.
get_property(IS_MULTICONFIG GLOBAL PROPERTY GENERATOR_IS_MULTI_CONFIG)
if(IS_MULTICONFIG)
  set(CMAKE_CONFIGURATION_TYPES "Debug;Profile;Release"
    CACHE STRING "" FORCE)
else()
  if(NOT CMAKE_BUILD_TYPE AND NOT CMAKE_CONFIGURATION_TYPES)
    set(CMAKE_BUILD_TYPE "Debug" CACHE
      STRING "Flutter build mode" FORCE)
    set_property(CACHE CMAKE_BUILD_TYPE PROPERTY STRINGS
      "Debug" "Profile" "Release")
  endif()
endif()
# Define settings for the Profile build mode.
set(CMAKE_EXE_LINKER_FLAGS_PROFILE "${CMAKE_EXE_LINKER_FLAGS_RELEASE}")
set(CMAKE_SHARED_LINKER_FLAGS_PROFILE "${CMAKE_SHARED_LINKER_FLAGS_RELEASE}")
set(CMAKE_C_FLAGS_PROFILE "${CMAKE_C_FLAGS_RELEASE}")
set(CMAKE_CXX_FLAGS_PROFILE "${CMAKE_CXX_FLAGS_RELEASE}")

# Use Unicode for all projects.
add_definitions(-DUNICODE -D_UNICODE)

# Compilation settings that should be applied to most targets.
#
# Be cautious about adding new options here, as plugins use this function by
# default. In most cases, you should add new options to specific targets instead
# of modifying this function.
function(APPLY_STANDARD_SETTINGS TARGET)
  target_compile_features(${TARGET} PUBLIC cxx_std_17)
  target_compile_options(${TARGET} PRIVATE /W4 /WX /wd4100)
  target_compile_options(${TARGET} PRIVATE /EHsc)
  target_compile_definitions(${TARGET} PRIVATE "_HAS_EXCEPTIONS=0")
  target_compile_definitions(${TARGET} PRIVATE "$<$<CONFIG:Debug>:_DEBUG>")
endfunction()

# Which Claktory this build is (README.md, "Flavors"):
# - real: Claktory, only from the release script (CLAKTORY_FLAVOR=real) in a
#   Release build;
# - dev: Claktory Dev for the dev Claktory, any build with
#   --dart-define=FACTORY_DEV=true;
# - debug: every other build (flutter run, a plain flutter build windows),
#   also shown as Claktory Dev, as on the Mac (#501).
# Flutter hands its --dart-define values to CMake base64-encoded, one by one,
# in DART_DEFINES; "RkFDVE9SWV9ERVY9dHJ1ZQ==" is FACTORY_DEV=true.
# Read in a function: the file sets the version with PARENT_SCOPE, which
# CMake warns about at the top level.
function(claktory_tool_environment OUT)
  set(config "${CMAKE_CURRENT_SOURCE_DIR}/flutter/ephemeral/generated_config.cmake")
  if(EXISTS "${config}")
    include("${config}")
  endif()
  set(${OUT} "${FLUTTER_TOOL_ENVIRONMENT}" PARENT_SCOPE)
endfunction()
claktory_tool_environment(CLAKTORY_TOOL_ENVIRONMENT)
set(CLAKTORY_WANTS_DEV OFF)
if("${CLAKTORY_TOOL_ENVIRONMENT}" MATCHES "RkFDVE9SWV9ERVY9dHJ1ZQ==")
  set(CLAKTORY_WANTS_DEV ON)
endif()
set(CLAKTORY_WANTS_REAL OFF)
if("$ENV{CLAKTORY_FLAVOR}" STREQUAL "real")
  set(CLAKTORY_WANTS_REAL ON)
elseif(NOT "$ENV{CLAKTORY_FLAVOR}" STREQUAL "")
  message(FATAL_ERROR "CLAKTORY_FLAVOR is \"$ENV{CLAKTORY_FLAVOR}\": only \"real\" (or nothing) is known")
endif()
if(CLAKTORY_WANTS_REAL AND CLAKTORY_WANTS_DEV)
  message(FATAL_ERROR "CLAKTORY_FLAVOR=real with FACTORY_DEV=true: a build is one Claktory or the other")
endif()

# Flutter library and tool build rules.
set(FLUTTER_MANAGED_DIR "${CMAKE_CURRENT_SOURCE_DIR}/flutter")
add_subdirectory(${FLUTTER_MANAGED_DIR})

# Application build; see runner/CMakeLists.txt.
add_subdirectory("runner")


# Generated plugin build rules, which manage building the plugins and adding
# them to the application.
include(flutter/generated_plugins.cmake)


# === Installation ===
# Support files are copied into place next to the executable, so that it can
# run in place. This is done instead of making a separate bundle (as on Linux)
# so that building and running from within Visual Studio will work.
set(BUILD_BUNDLE_DIR "$<TARGET_FILE_DIR:${BINARY_NAME}>")
# Make the "install" step default, as it's required to run.
set(CMAKE_VS_INCLUDE_INSTALL_TO_DEFAULT_BUILD 1)
if(CMAKE_INSTALL_PREFIX_INITIALIZED_TO_DEFAULT)
  set(CMAKE_INSTALL_PREFIX "${BUILD_BUNDLE_DIR}" CACHE PATH "..." FORCE)
endif()

set(INSTALL_BUNDLE_DATA_DIR "${CMAKE_INSTALL_PREFIX}/data")
set(INSTALL_BUNDLE_LIB_DIR "${CMAKE_INSTALL_PREFIX}")

install(TARGETS ${BINARY_NAME} RUNTIME DESTINATION "${CMAKE_INSTALL_PREFIX}"
  COMPONENT Runtime)

install(FILES "${FLUTTER_ICU_DATA_FILE}" DESTINATION "${INSTALL_BUNDLE_DATA_DIR}"
  COMPONENT Runtime)

install(FILES "${FLUTTER_LIBRARY}" DESTINATION "${INSTALL_BUNDLE_LIB_DIR}"
  COMPONENT Runtime)

# The pictures Windows shows on the app's notifications (its AppUserModelId
# registration, runner/notifications_channel.cpp): Claktory's and Claktory
# Dev's, beside the app.
install(FILES
  "${CMAKE_CURRENT_SOURCE_DIR}/runner/resources/notification_icon.png"
  "${CMAKE_CURRENT_SOURCE_DIR}/runner/resources/notification_icon_dev.png"
  DESTINATION "${INSTALL_BUNDLE_DATA_DIR}" COMPONENT Runtime)

if(PLUGIN_BUNDLED_LIBRARIES)
  install(FILES "${PLUGIN_BUNDLED_LIBRARIES}"
    DESTINATION "${INSTALL_BUNDLE_LIB_DIR}"
    COMPONENT Runtime)
endif()

# The Visual C++ runtime beside the app, app-local as Microsoft allows:
# msvcp140, vcruntime140 and vcruntime140_1, which the app, its plugin and the
# tunnel's library (claktory_tunnel.dll) load, and which a clean Windows
# doesn't have. Not the UCRT, which is part of Windows 10 and later. Profile
# and Release only: a Debug build uses the debug runtime, which only Visual
# Studio has and which can't be shipped.
set(CMAKE_INSTALL_UCRT_LIBRARIES OFF)
set(CMAKE_INSTALL_SYSTEM_RUNTIME_LIBS_SKIP TRUE)
include(InstallRequiredSystemLibraries)
# Only the three the binaries import (dumpbin /dependents), not the rest of
# the redistributable (concrt140, msvcp140_1, _2, _atomic_wait…).
list(FILTER CMAKE_INSTALL_SYSTEM_RUNTIME_LIBS INCLUDE
  REGEX "/(msvcp140|vcruntime140|vcruntime140_1)\\.dll$")
install(PROGRAMS ${CMAKE_INSTALL_SYSTEM_RUNTIME_LIBS}
  DESTINATION "${INSTALL_BUNDLE_LIB_DIR}"
  CONFIGURATIONS Profile;Release
  COMPONENT Runtime)

# Copy the native assets provided by the hook/build.dart from all packages.
set(NATIVE_ASSETS_DIR "${PROJECT_BUILD_DIR}native_assets/windows/")
install(DIRECTORY "${NATIVE_ASSETS_DIR}"
   DESTINATION "${INSTALL_BUNDLE_LIB_DIR}"
   COMPONENT Runtime)

# Fully re-copy the assets directory on each build to avoid having stale files
# from a previous install.
set(FLUTTER_ASSET_DIR_NAME "flutter_assets")
install(CODE "
  file(REMOVE_RECURSE \"${INSTALL_BUNDLE_DATA_DIR}/${FLUTTER_ASSET_DIR_NAME}\")
  " COMPONENT Runtime)
install(DIRECTORY "${PROJECT_BUILD_DIR}/${FLUTTER_ASSET_DIR_NAME}"
  DESTINATION "${INSTALL_BUNDLE_DATA_DIR}" COMPONENT Runtime)

# Install the AOT library on non-Debug builds only.
install(FILES "${AOT_LIBRARY}" DESTINATION "${INSTALL_BUNDLE_DATA_DIR}"
  CONFIGURATIONS Profile;Release
  COMPONENT Runtime)
