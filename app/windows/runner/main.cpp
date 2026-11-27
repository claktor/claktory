#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>
#include <windows.h>
#include <shobjidl.h>

#include <string>
#include <utility>
#include <vector>

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
