#ifndef RUNNER_APP_PATHS_H_
#define RUNNER_APP_PATHS_H_

#include <optional>
#include <string>

// This device's own folder for the app (windows.md section 1, "Device-only
// app data"): %LOCALAPPDATA%\Claktory\app (app-dev for Claktory Dev, app-debug
// for other builds, flavor.h), apart from the backend's data beside it. It
// holds only the device-local state tenet 7 allows: the connections (DPAPI),
// unsent drafts and the window's place. Local, never roaming: it never follows
// the person to another PC, and OneDrive doesn't sync it.
//
// Made on first use, owned by this Windows account, with a protected DACL
// that lets in only this account and SYSTEM (nothing inherited from above),
// so another account on the PC can't list or read it. Administrators can
// still take ownership: it's no protection from them. A folder already there
// is used only if it's a plain folder (no junction or link) owned by this
// account, and its DACL is made the private one again. Nullopt, with the
// reason logged, when the folder can't be found, made or taken.
std::optional<std::wstring> AppDataFolder();

// [name] inside AppDataFolder().
std::optional<std::wstring> AppDataFile(const wchar_t* name);

#endif  // RUNNER_APP_PATHS_H_
