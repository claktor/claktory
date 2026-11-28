#include "setup_channel.h"

#include <flutter/standard_method_codec.h>
#include <knownfolders.h>
#include <shlobj.h>

#include <algorithm>
#include <iterator>
#include <thread>
#include <utility>
#include <variant>
#include <vector>

#include "app_messages.h"
#include "app_paths.h"
#include "core/command_line.h"
#include "file_store.h"
#include "flavor.h"
#include "utils.h"

using flutter::EncodableList;
using flutter::EncodableMap;
using flutter::EncodableValue;

namespace {

// The file that says this PC has no Claktory of its own, in the app's
// folder; its being there is the answer.
constexpr wchar_t kAppOnlyFile[] = L"app-only";

// The most of a run's output kept, each of stdout and stderr: the Report is
// one line, and anything past this is noise.
constexpr size_t kMostOutput = 1 << 20;

bool IsFile(const std::wstring& path) {
  DWORD attributes = ::GetFileAttributesW(path.c_str());
  return attributes != INVALID_FILE_ATTRIBUTES &&
         !(attributes & FILE_ATTRIBUTE_DIRECTORY);
}

// This PC's Claktory's home, as `factoryd install` sets it up:
// %LOCALAPPDATA%\Claktory\factory, factory-dev for Claktory Dev, which
// talks to the dev Claktory. Never FACTORY_HOME: the sign-in task starts the
// backend without this app's environment, so install doesn't read it.
std::wstring BackendHome() {
  PWSTR local = nullptr;
  std::wstring home;
  if (SUCCEEDED(::SHGetKnownFolderPath(FOLDERID_LocalAppData, KF_FLAG_DEFAULT,
                                       nullptr, &local)) &&
      local != nullptr) {
    home = std::wstring(local) + L"\\Claktory\\" +
           (CLAKTORY_FLAVOR_DEV ? L"factory-dev" : L"factory");
  }
  if (local != nullptr) ::CoTaskMemFree(local);
  return home;
}

// The backend the person's Claktory runs: <home>\bin\factoryd.exe once set
// up; before that, the one installed beside the app (<root>\backend), which
// `install` copies into the home. "" when there's neither.
std::wstring Factoryd() {
  std::wstring home = BackendHome();
  if (!home.empty() && IsFile(home + L"\\bin\\factoryd.exe")) {
    return home + L"\\bin\\factoryd.exe";
  }
  return std::wstring();
}

std::wstring Carried() {
  std::wstring launcher = InstalledLauncher();
  if (launcher.empty()) return std::wstring();
  std::wstring root = launcher.substr(0, launcher.find_last_of(L'\\'));
  std::wstring carried = root + L"\\backend\\factoryd.exe";
  return IsFile(carried) ? carried : std::wstring();
}

std::string ModulePath() {
  wchar_t module[MAX_PATH * 4];
  DWORD length = ::GetModuleFileNameW(nullptr, module,
                                      static_cast<DWORD>(std::size(module)));
  if (length == 0 || length >= std::size(module)) return std::string();
  return Utf8FromUtf16(std::wstring(module, length).c_str());
}

// Reads [pipe] to its end, keeping up to kMostOutput of it.
std::string ReadAll(HANDLE pipe) {
  std::string all;
  char buffer[4096];
  DWORD read = 0;
  while (::ReadFile(pipe, buffer, sizeof(buffer), &read, nullptr) && read > 0) {
    if (all.size() < kMostOutput) {
      all.append(buffer, std::min<size_t>(read, kMostOutput - all.size()));
    }
  }
  return all;
}

}  // namespace

SetupChannel::SetupChannel(flutter::BinaryMessenger* messenger, HWND window)
    : window_(window) {
  channel_ = std::make_unique<flutter::MethodChannel<EncodableValue>>(
      messenger, "factory/setup",
      &flutter::StandardMethodCodec::GetInstance());
  channel_->SetMethodCallHandler(
      [this](const auto& call, auto result) { Handle(call, std::move(result)); });
}

SetupChannel::~SetupChannel() {
  channel_->SetMethodCallHandler(nullptr);
}

void SetupChannel::Handle(
    const flutter::MethodCall<EncodableValue>& call,
    std::unique_ptr<flutter::MethodResult<EncodableValue>> result) {
  const std::string& method = call.method_name();
  if (method == "location") {
    result->Success(EncodableValue(EncodableMap{
        {EncodableValue("path"), EncodableValue(ModulePath())},
        {EncodableValue("needsMove"), EncodableValue(false)},
        {EncodableValue("fromImage"), EncodableValue(false)},
        {EncodableValue("helpers"),
         EncodableValue(!Factoryd().empty() || !Carried().empty())},
    }));
    return;
  }
  if (method == "install" || method == "uninstall") {
    std::vector<std::string> arguments;
    if (const auto* list = std::get_if<EncodableList>(call.arguments())) {
      for (const auto& each : *list) {
        if (const auto* text = std::get_if<std::string>(&each)) {
          arguments.push_back(*text);
        } else {
          result->Error("arguments", "arguments are text");
          return;
        }
      }
    }
    if (arguments.empty() || arguments.front() != method) {
      result->Error("arguments", "only " + method);
      return;
    }
#if CLAKTORY_FLAVOR_DEV
    // Claktory Dev sets up, and removes, the dev Claktory.
    arguments.push_back("--dev");
#endif
    // Set up, the home's own; before that, the one beside the app, which
    // puts itself in the home.
    std::wstring exe = Factoryd();
    if (exe.empty()) exe = Carried();
    if (exe.empty()) {
      result->Error("missing", "no factoryd.exe");
      return;
    }
    Start(exe, arguments, std::move(result));
    return;
  }
  if (method == "appOnly") {
    auto path = AppDataFile(kAppOnlyFile);
    result->Success(EncodableValue(path.has_value() && IsFile(*path)));
    return;
  }
  if (method == "setAppOnly") {
    const auto* on = std::get_if<bool>(call.arguments());
    auto path = AppDataFile(kAppOnlyFile);
    if (on == nullptr || !path) {
      result->Error("setAppOnly", "can't keep it");
      return;
    }
    auto error = *on ? WriteWholeFile(*path, "1") : DeleteFileIfThere(*path);
    if (error) {
      result->Error("setAppOnly", *error);
      return;
    }
    result->Success();
    return;
  }
  if (method == "backgroundStatus") {
    result->Success(EncodableValue("unavailable"));
    return;
  }
  if (method == "openLoginItems") {
    result->Success();
    return;
  }
  result->NotImplemented();
}

void SetupChannel::Start(
    const std::wstring& exe, const std::vector<std::string>& arguments,
    std::unique_ptr<flutter::MethodResult<EncodableValue>> result) {
  auto run = std::make_shared<Run>();
  run->result = std::move(result);
  WPARAM id;
  {
    std::lock_guard<std::mutex> lock(runs_lock_);
    id = next_run_++;
    runs_[id] = run;
  }
  std::vector<std::wstring> wide;
  for (const auto& argument : arguments) wide.push_back(Utf16FromUtf8(argument));
  std::wstring line = claktory::CommandLine(exe, wide);
  HWND window = window_;
  std::thread([run, id, exe, line, window]() mutable {
    SECURITY_ATTRIBUTES inherit{sizeof(inherit), nullptr, TRUE};
    HANDLE out_read = nullptr, out_write = nullptr;
    HANDLE err_read = nullptr, err_write = nullptr;
    if (!::CreatePipe(&out_read, &out_write, &inherit, 0) ||
        !::CreatePipe(&err_read, &err_write, &inherit, 0)) {
      run->failure = Win32ErrorMessage(::GetLastError());
      ::PostMessageW(window, kSetupDoneMessage, id, 0);
      return;
    }
    // Only the child's ends are inherited.
    ::SetHandleInformation(out_read, HANDLE_FLAG_INHERIT, 0);
    ::SetHandleInformation(err_read, HANDLE_FLAG_INHERIT, 0);
    HANDLE nothing = ::CreateFileW(L"NUL", GENERIC_READ, FILE_SHARE_READ,
                                   &inherit, OPEN_EXISTING, 0, nullptr);
    STARTUPINFOEXW startup{};
    startup.StartupInfo.cb = sizeof(startup);
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = nothing;
    startup.StartupInfo.hStdOutput = out_write;
    startup.StartupInfo.hStdError = err_write;
    // Only these three handles go to the child, whatever else this process
    // has open and inheritable.
    HANDLE handles[] = {nothing, out_write, err_write};
    SIZE_T size = 0;
    ::InitializeProcThreadAttributeList(nullptr, 1, 0, &size);
    std::vector<char> attributes(size);
    auto list =
        reinterpret_cast<LPPROC_THREAD_ATTRIBUTE_LIST>(attributes.data());
    bool listed =
        ::InitializeProcThreadAttributeList(list, 1, 0, &size) &&
        ::UpdateProcThreadAttribute(list, 0,
                                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, handles,
                                    sizeof(handles), nullptr, nullptr);
    startup.lpAttributeList = listed ? list : nullptr;
    PROCESS_INFORMATION process{};
    BOOL started = listed && ::CreateProcessW(
        exe.c_str(), line.data(), nullptr, nullptr, TRUE,
        CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT |
            EXTENDED_STARTUPINFO_PRESENT,
        nullptr, nullptr, &startup.StartupInfo, &process);
    DWORD failed = started ? 0 : ::GetLastError();
    if (listed) ::DeleteProcThreadAttributeList(list);
    ::CloseHandle(out_write);
    ::CloseHandle(err_write);
    if (nothing != INVALID_HANDLE_VALUE) ::CloseHandle(nothing);
    if (!started) {
      run->failure = Win32ErrorMessage(failed);
    } else {
      ::CloseHandle(process.hThread);
      // Both read before waiting, so a full pipe can't stall it.
      std::string err;
      std::thread errors([&err, err_read]() { err = ReadAll(err_read); });
      run->out = ReadAll(out_read);
      errors.join();
      run->err = std::move(err);
      ::WaitForSingleObject(process.hProcess, INFINITE);
      DWORD code = 0;
      ::GetExitCodeProcess(process.hProcess, &code);
      ::CloseHandle(process.hProcess);
      run->code = code;
      run->started = true;
    }
    ::CloseHandle(out_read);
    ::CloseHandle(err_read);
    ::PostMessageW(window, kSetupDoneMessage, id, 0);
  }).detach();
}

void SetupChannel::Done(WPARAM id) {
  std::shared_ptr<Run> run;
  {
    std::lock_guard<std::mutex> lock(runs_lock_);
    auto found = runs_.find(id);
    if (found == runs_.end()) return;
    run = found->second;
    runs_.erase(found);
  }
  if (!run->started) {
    run->result->Error("start", run->failure);
    return;
  }
  run->result->Success(EncodableValue(EncodableMap{
      {EncodableValue("code"), EncodableValue(static_cast<int>(run->code))},
      {EncodableValue("stdout"), EncodableValue(run->out)},
      {EncodableValue("stderr"), EncodableValue(run->err)},
  }));
}
