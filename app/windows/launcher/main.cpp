// The Start menu's Claktory: %LOCALAPPDATA%\Programs\Claktory\Claktory.exe,
// a small program with no window of its own that starts the app's version
// app\current names (core/launcher_plan.h) and steps aside. A running app's
// folder can't be replaced, so each version has its own, and an update
// unpacks the next beside it and points app\current at it once it's
// verified; the next start is the new version, and no agent restarts.
//
// - It starts only what app\current names, never a folder it picked
//   itself; with no pointer it can read, it says Claktory needs installing
//   again rather than guessing.
// - It passes on its own command line (a link, say), starts the app in its
//   own folder, and lets it take the foreground. An app that just updated
//   starts it with "--after <its pid>" and quits; it waits for that one to
//   go first, so the new app doesn't find the old one's window and hand
//   over to it.
// - When that version fails to start (it can't be started, or it exits
//   with a failure within kWaitMs), it points app\current at app\previous,
//   the version the updater replaced, and starts that one.
// - Once the app runs, it deletes the versions neither pointer names,
//   leaving any it can't (in use) for next time, and nothing while an
//   update holds app\installing. It never follows a link out of app\, and
//   never deletes anything whose name isn't a version. The launcher an
//   update moved aside, Claktory.exe.old, goes too.
//
// Its own log, app\launcher.log, says what it started and why.

#include <windows.h>

#include <cstdio>
#include <string>
#include <vector>

#include "core/launcher_plan.h"
#include "flavor.h"

namespace {

// How long a version has to fail before it counts as started.
constexpr DWORD kWaitMs = 5000;

// The longest it waits for the app that started it to quit.
constexpr DWORD kAfterWaitMs = 30000;

// The log's most, past which it starts again.
constexpr LONGLONG kMostLogBytes = 256 * 1024;

std::wstring OwnFolder() {
  std::wstring path(MAX_PATH, L'\0');
  for (;;) {
    DWORD length = ::GetModuleFileNameW(nullptr, path.data(),
                                        static_cast<DWORD>(path.size()));
    if (length == 0) return std::wstring();
    if (length < path.size()) {
      path.resize(length);
      break;
    }
    path.resize(path.size() * 2);
  }
  return path.substr(0, path.find_last_of(L'\\'));
}

// A version's ASCII, widened.
std::wstring Wide(const std::string& ascii) {
  return std::wstring(ascii.begin(), ascii.end());
}

void Log(const std::wstring& app, const std::string& message) {
  std::wstring path = app + L"\\launcher.log";
  WIN32_FILE_ATTRIBUTE_DATA data{};
  bool full = ::GetFileAttributesExW(path.c_str(), GetFileExInfoStandard,
                                     &data) &&
              ((static_cast<LONGLONG>(data.nFileSizeHigh) << 32) |
               data.nFileSizeLow) > kMostLogBytes;
  HANDLE file = ::CreateFileW(path.c_str(),
                              full ? GENERIC_WRITE : FILE_APPEND_DATA,
                              FILE_SHARE_READ, nullptr,
                              full ? CREATE_ALWAYS : OPEN_ALWAYS,
                              FILE_ATTRIBUTE_NORMAL, nullptr);
  if (file == INVALID_HANDLE_VALUE) return;
  SYSTEMTIME now{};
  ::GetLocalTime(&now);
  char stamp[32];
  std::snprintf(stamp, sizeof(stamp), "%04u-%02u-%02u %02u:%02u:%02u ",
                now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute,
                now.wSecond);
  std::string line = stamp + message + "\r\n";
  DWORD written = 0;
  ::WriteFile(file, line.data(), static_cast<DWORD>(line.size()), &written,
              nullptr);
  ::CloseHandle(file);
}

// The version a pointer in [app] names; "" when it's missing or isn't one.
std::string ReadPointer(const std::wstring& app, const wchar_t* name) {
  std::wstring path = app + L"\\" + name;
  HANDLE file = ::CreateFileW(path.c_str(), GENERIC_READ,
                              FILE_SHARE_READ | FILE_SHARE_WRITE |
                                  FILE_SHARE_DELETE,
                              nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL,
                              nullptr);
  if (file == INVALID_HANDLE_VALUE) return std::string();
  // A version and a line break fit; anything longer isn't a pointer.
  char bytes[claktory::kMostVersionLength + 8];
  DWORD read = 0;
  BOOL done = ::ReadFile(file, bytes, sizeof(bytes), &read, nullptr);
  ::CloseHandle(file);
  if (!done || read == sizeof(bytes)) return std::string();
  return claktory::PointedVersion(std::string(bytes, read));
}

// Points [name] in [app] at [version], all at once: a file beside it,
// then moved over it.
bool WritePointer(const std::wstring& app, const wchar_t* name,
                  const std::string& version) {
  std::wstring path = app + L"\\" + name;
  std::wstring next =
      path + L".next-" + std::to_wstring(::GetCurrentProcessId());
  HANDLE file = ::CreateFileW(next.c_str(), GENERIC_WRITE, 0, nullptr,
                              CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
  if (file == INVALID_HANDLE_VALUE) return false;
  std::string text = version + "\n";
  DWORD written = 0;
  bool done = ::WriteFile(file, text.data(), static_cast<DWORD>(text.size()),
                          &written, nullptr) &&
              written == text.size() && ::FlushFileBuffers(file);
  ::CloseHandle(file);
  done = done && ::MoveFileExW(next.c_str(), path.c_str(),
                               MOVEFILE_REPLACE_EXISTING |
                                   MOVEFILE_WRITE_THROUGH);
  if (!done) ::DeleteFileW(next.c_str());
  return done;
}

// The launcher's own arguments, as typed: its command line past its name.
std::wstring OwnArguments() {
  const wchar_t* line = ::GetCommandLineW();
  if (*line == L'"') {
    ++line;
    while (*line != L'\0' && *line != L'"') ++line;
    if (*line == L'"') ++line;
  } else {
    while (*line != L'\0' && *line != L' ' && *line != L'\t') ++line;
  }
  while (*line == L' ' || *line == L'\t') ++line;
  return line;
}

// Starts [version]'s app with [arguments]: true once it runs (or handed
// over to the one already running), false when it failed to start.
bool Start(const std::wstring& app, const std::string& version,
           const std::wstring& arguments) {
  std::wstring folder = app + L"\\" + Wide(version);
  std::wstring exe = folder + L"\\Claktory.exe";
  DWORD attributes = ::GetFileAttributesW(exe.c_str());
  if (attributes == INVALID_FILE_ATTRIBUTES ||
      (attributes & FILE_ATTRIBUTE_DIRECTORY)) {
    Log(app, version + ": no Claktory.exe");
    return false;
  }
  std::wstring command = L"\"" + exe + L"\"";
  if (!arguments.empty()) command += L" " + arguments;
  STARTUPINFOW startup{};
  startup.cb = sizeof(startup);
  PROCESS_INFORMATION process{};
  if (!::CreateProcessW(exe.c_str(), command.data(), nullptr, nullptr, FALSE,
                        0, nullptr, folder.c_str(), &startup, &process)) {
    Log(app, version + ": couldn't start (error " +
                 std::to_string(::GetLastError()) + ")");
    return false;
  }
  ::CloseHandle(process.hThread);
  bool exited = ::WaitForSingleObject(process.hProcess, kWaitMs) ==
                WAIT_OBJECT_0;
  DWORD code = 0;
  if (exited) ::GetExitCodeProcess(process.hProcess, &code);
  ::CloseHandle(process.hProcess);
  if (claktory::FailedToStart(exited, code)) {
    char hex[16];
    std::snprintf(hex, sizeof(hex), "0x%08lX", static_cast<unsigned long>(code));
    Log(app, version + ": exited at once with " + hex);
    return false;
  }
  Log(app, "started " + version);
  return true;
}

// Deletes [path], a folder or a file; a link (a junction, say) is removed
// itself, never followed. What can't be deleted (in use) stays.
void DeleteTree(const std::wstring& path) {
  DWORD attributes = ::GetFileAttributesW(path.c_str());
  if (attributes == INVALID_FILE_ATTRIBUTES) return;
  if (attributes & FILE_ATTRIBUTE_READONLY) {
    ::SetFileAttributesW(path.c_str(),
                         attributes & ~static_cast<DWORD>(
                                          FILE_ATTRIBUTE_READONLY));
  }
  if (!(attributes & FILE_ATTRIBUTE_DIRECTORY)) {
    ::DeleteFileW(path.c_str());
    return;
  }
  if (!(attributes & FILE_ATTRIBUTE_REPARSE_POINT)) {
    WIN32_FIND_DATAW found{};
    HANDLE search = ::FindFirstFileExW((path + L"\\*").c_str(),
                                       FindExInfoBasic, &found,
                                       FindExSearchNameMatch, nullptr, 0);
    if (search != INVALID_HANDLE_VALUE) {
      do {
        std::wstring name = found.cFileName;
        if (name == L"." || name == L"..") continue;
        DeleteTree(path + L"\\" + name);
      } while (::FindNextFileW(search, &found));
      ::FindClose(search);
    }
  }
  ::RemoveDirectoryW(path.c_str());
}

// Deletes the versions in [app] neither [current] nor [previous] names,
// unless an update is laying one down (it holds app\installing open until
// app\current names it).
void DeleteStale(const std::wstring& app, const std::string& current,
                 const std::string& previous) {
  if (::GetFileAttributesW((app + L"\\installing").c_str()) !=
      INVALID_FILE_ATTRIBUTES) {
    Log(app, "an update is installing: nothing deleted");
    return;
  }
  std::vector<std::string> folders;
  WIN32_FIND_DATAW found{};
  HANDLE search = ::FindFirstFileExW((app + L"\\*").c_str(), FindExInfoBasic,
                                     &found, FindExSearchNameMatch, nullptr,
                                     0);
  if (search == INVALID_HANDLE_VALUE) return;
  do {
    if (!(found.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY)) continue;
    std::wstring name = found.cFileName;
    std::string ascii;
    bool plain = true;
    for (wchar_t c : name) {
      if (c > 0x7F) plain = false;
      ascii.push_back(static_cast<char>(c));
    }
    // A version is ASCII: anything else is never one of them.
    if (plain) folders.push_back(ascii);
  } while (::FindNextFileW(search, &found));
  ::FindClose(search);
  for (const auto& stale : claktory::StaleVersions(folders, current, previous)) {
    Log(app, "deleting " + stale);
    DeleteTree(app + L"\\" + Wide(stale));
  }
}

// Waits for process [pid] to end, at most kAfterWaitMs.
void WaitFor(const std::wstring& app, unsigned long pid) {
  HANDLE process = ::OpenProcess(SYNCHRONIZE, FALSE, pid);
  if (process == nullptr) return;
  if (::WaitForSingleObject(process, kAfterWaitMs) != WAIT_OBJECT_0) {
    Log(app, "the app before is still running after " +
                 std::to_string(kAfterWaitMs / 1000) + " s");
  }
  ::CloseHandle(process);
}

void Tell(const wchar_t* text) {
  ::MessageBoxW(nullptr, text, flavor::kAppName, MB_OK | MB_ICONERROR);
}

}  // namespace

int APIENTRY wWinMain(_In_ HINSTANCE, _In_opt_ HINSTANCE, _In_ wchar_t*,
                      _In_ int) {
  // The app it starts comes to the front, as if the person had opened it.
  ::AllowSetForegroundWindow(ASFW_ANY);
  std::wstring root = OwnFolder();
  if (root.empty()) return EXIT_FAILURE;
  std::wstring app = root + L"\\app";
  std::string current = ReadPointer(app, L"current");
  std::string previous = ReadPointer(app, L"previous");
  if (current.empty()) {
    Log(app, "app\\current names no version");
    Tell(L"Claktory can't start because some of its files are missing. "
         L"Install Claktory again to fix it.");
    return EXIT_FAILURE;
  }
  std::wstring arguments = OwnArguments();
  if (unsigned long after = claktory::TakeAfter(&arguments)) {
    WaitFor(app, after);
  }
  ::DeleteFileW((root + L"\\Claktory.exe.old").c_str());
  if (Start(app, current, arguments)) {
    DeleteStale(app, current, previous);
    return EXIT_SUCCESS;
  }
  if (previous.empty() || previous == current) {
    Tell(L"Claktory couldn't start. Install Claktory again to fix it.");
    return EXIT_FAILURE;
  }
  // Back to the version the updater replaced, which it had checked too.
  Log(app, "going back from " + current + " to " + previous);
  if (!WritePointer(app, L"current", previous)) {
    Log(app, "couldn't point app\\current at " + previous);
  }
  if (Start(app, previous, arguments)) return EXIT_SUCCESS;
  Tell(L"Claktory couldn't start. Install Claktory again to fix it.");
  return EXIT_FAILURE;
}
