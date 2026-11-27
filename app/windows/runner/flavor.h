#ifndef RUNNER_FLAVOR_H_
#define RUNNER_FLAVOR_H_

// Which Claktory this build is: the real one, Claktory Dev for the dev
// Claktory, or any other build (shown as Claktory Dev too, #501). CMake sets
// CLAKTORY_FLAVOR_REAL and CLAKTORY_FLAVOR_DEV (runner/CMakeLists.txt); the
// names below keep the Mac's internal ids (dev.factory.app...) where Windows
// wants an id, and say Claktory wherever the person reads it.

#ifndef CLAKTORY_FLAVOR_REAL
#define CLAKTORY_FLAVOR_REAL 0
#endif
#ifndef CLAKTORY_FLAVOR_DEV
#define CLAKTORY_FLAVOR_DEV 0
#endif

namespace flavor {

#if CLAKTORY_FLAVOR_REAL
// What the person reads: the window's title, the taskbar.
constexpr wchar_t kAppName[] = L"Claktory";
// The app's own id: its AppUserModelID (taskbar grouping, notifications),
// its window class and its single-instance mutex. The Mac's bundle id.
constexpr wchar_t kAppId[] = L"dev.factory.app";
// This device's own folder for the app, in %LOCALAPPDATA%\Claktory
// (windows.md section 1: device-only app data), apart from the backend's.
constexpr wchar_t kDataFolder[] = L"app";
// The COM class Windows starts to tell the app of a toast clicked while it
// isn't running (toast_activator.h); one per flavor, as each is its own exe.
constexpr wchar_t kToastActivator[] = L"{B92913F3-A773-47B2-93D2-0F951B7A1399}";
constexpr bool kIsReal = true;
#elif CLAKTORY_FLAVOR_DEV
constexpr wchar_t kAppName[] = L"Claktory Dev";
constexpr wchar_t kAppId[] = L"dev.factory.app.dev";
constexpr wchar_t kDataFolder[] = L"app-dev";
constexpr wchar_t kToastActivator[] = L"{8058C1B5-FD67-439A-882C-5A7F685812FE}";
constexpr bool kIsReal = false;
#else
constexpr wchar_t kAppName[] = L"Claktory Dev";
constexpr wchar_t kAppId[] = L"dev.factory.app.debug";
constexpr wchar_t kDataFolder[] = L"app-debug";
constexpr wchar_t kToastActivator[] = L"{B376FC2C-E72D-4024-AA71-68CD59CF51BC}";
constexpr bool kIsReal = false;
#endif

}  // namespace flavor

#endif  // RUNNER_FLAVOR_H_
