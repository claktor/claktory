#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>
#include <windows.h>
#include <propkey.h>
#include <propvarutil.h>
#include <shellapi.h>
#include <shobjidl.h>

#include <cwchar>
#include <string>
#include <utility>
#include <vector>

#include "app_paths.h"
#include "flavor.h"
#include "flutter_window.h"
#include "toast_activator.h"
#include "utils.h"
#include "window_placement.h"

namespace {

// One app per Windows session: a second launch (the Start menu clicked again,
// say) brings the window already open to the front and leaves. The mutex is
// in this session's namespace, so another account signed in on the same PC
// has its own.
bool AnotherIsRunning(HANDLE* mutex) {
  std::wstring name = std::wstring(L"Local\\") + flavor::kAppId;
  *mutex = ::CreateMutexW(nullptr, TRUE, name.c_str());
  if (*mutex != nullptr && ::GetLastError() != ERROR_ALREADY_EXISTS) {
    return false;
  }
  HWND window = ::FindWindowW(flavor::kAppId, nullptr);
  if (window != nullptr) {
    if (::IsIconic(window)) ::ShowWindow(window, SW_RESTORE);
    ::SetForegroundWindow(window);
  }
  return true;
}

// When this app runs from app\<version>\ beside the launcher
// (../launcher/main.cpp; windows-updates.md section 5), pinning its window
// to the taskbar pins the launcher, the Start menu's Claktory, never this
// version's own exe, which a later update deletes. Anywhere else (a build,
// an unzipped bundle), the window is left as Windows makes it.
void PinTheLauncher(HWND window) {
  std::wstring launcher = InstalledLauncher();
  if (launcher.empty()) {
    Log("no launcher beside app\\: the taskbar pins this exe");
    return;
  }
  IPropertyStore* store = nullptr;
  HRESULT got = ::SHGetPropertyStoreForWindow(window, IID_PPV_ARGS(&store));
  if (FAILED(got)) {
    Log("the window's properties can't be set: " +
        Win32ErrorMessage(static_cast<unsigned long>(got)));
    return;
  }
  HRESULT failed = S_OK;
  auto set = [store, &failed](const PROPERTYKEY& key,
                              const std::wstring& value) {
    PROPVARIANT variant;
    HRESULT done = ::InitPropVariantFromString(value.c_str(), &variant);
    if (SUCCEEDED(done)) {
      done = store->SetValue(key, variant);
      ::PropVariantClear(&variant);
    }
    if (FAILED(done) && SUCCEEDED(failed)) failed = done;
  };
  // The relaunch properties first, the id last: Windows reads them when
  // the id is set (Microsoft's "Application User Model IDs").
  set(PKEY_AppUserModel_RelaunchCommand, L"\"" + launcher + L"\"");
  set(PKEY_AppUserModel_RelaunchDisplayNameResource, flavor::kAppName);
  set(PKEY_AppUserModel_RelaunchIconResource, launcher + L",0");
  set(PKEY_AppUserModel_ID, flavor::kAppId);
  if (SUCCEEDED(failed)) failed = store->Commit();
  store->Release();
  if (FAILED(failed)) {
    Log("the taskbar can't be pointed at the launcher: " +
        Win32ErrorMessage(static_cast<unsigned long>(failed)));
  } else {
    Log("the taskbar pins the launcher, " + Utf8FromUtf16(launcher.c_str()));
  }
}

}  // namespace

int APIENTRY wWinMain(_In_ HINSTANCE instance, _In_opt_ HINSTANCE prev,
                      _In_ wchar_t *command_line, _In_ int show_command) {
  // Attach to console when present (e.g., 'flutter run') or create a
  // new console when running with a debugger.
  if (!::AttachConsole(ATTACH_PARENT_PROCESS) && ::IsDebuggerPresent()) {
    CreateAndAttachConsole();
  }

  HANDLE mutex = nullptr;
  if (AnotherIsRunning(&mutex)) {
    if (mutex != nullptr) ::CloseHandle(mutex);
    return EXIT_SUCCESS;
  }

  // One line on every start, so a test run's log always shows the app ran
  // and which one it was.
  Log("starting " + Utf8FromUtf16(flavor::kAppName) + " (" +
      Utf8FromUtf16(flavor::kAppId) + ")");

  // The app's own id for the taskbar and, later, its notifications: the
  // installer's Start menu shortcut carries the same one (README.md).
  ::SetCurrentProcessExplicitAppUserModelID(flavor::kAppId);

  // OLE, not just COM: dropping files on the window needs it
  // (drop_target.h), and it's COM's single-threaded apartment besides.
  bool ole = SUCCEEDED(::OleInitialize(nullptr));
  if (!ole) {
    Log("OLE didn't start: files can't be dropped on the window");
    ::CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
  }

  flutter::DartProject project(L"data");

  std::vector<std::string> command_line_arguments;
  for (auto& argument : GetCommandLineArguments()) {
    // Windows started the app for a toast clicked (toast_activator.h): the
    // click itself comes through COM once the window is up.
    if (IsComStartArgument(argument)) continue;
    command_line_arguments.push_back(std::move(argument));
  }

  project.set_dart_entrypoint_arguments(std::move(command_line_arguments));

  FlutterWindow window(project);
  bool maximized = false;
  RECT frame = InitialWindowFrame(&maximized);
  if (!window.Create(flavor::kAppName, frame, maximized)) {
    return EXIT_FAILURE;
  }
  window.SetQuitOnClose(true);
  PinTheLauncher(window.GetHandle());

  ::MSG msg;
  while (::GetMessage(&msg, nullptr, 0, 0)) {
    ::TranslateMessage(&msg);
    ::DispatchMessage(&msg);
  }

  if (ole) {
    ::OleUninitialize();
  } else {
    ::CoUninitialize();
  }
  if (mutex != nullptr) ::CloseHandle(mutex);
  return EXIT_SUCCESS;
}
