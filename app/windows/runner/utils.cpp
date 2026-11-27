#include "utils.h"

#include <flutter_windows.h>
#include <io.h>
#include <stdio.h>
#include <windows.h>

#include <climits>
#include <cstdio>
#include <iostream>
#include <iterator>
#include <string>

void CreateAndAttachConsole() {
  if (::AllocConsole()) {
    FILE *unused;
    if (freopen_s(&unused, "CONOUT$", "w", stdout)) {
      _dup2(_fileno(stdout), 1);
    }
    if (freopen_s(&unused, "CONOUT$", "w", stderr)) {
      _dup2(_fileno(stdout), 2);
    }
    std::ios::sync_with_stdio();
    FlutterDesktopResyncOutputStreams();
  }
}

std::vector<std::string> GetCommandLineArguments() {
  // Convert the UTF-16 command line arguments to UTF-8 for the Engine to use.
  int argc;
  wchar_t** argv = ::CommandLineToArgvW(::GetCommandLineW(), &argc);
  if (argv == nullptr) {
    return std::vector<std::string>();
  }

  std::vector<std::string> command_line_arguments;

  // Skip the first argument as it's the binary name.
  for (int i = 1; i < argc; i++) {
    command_line_arguments.push_back(Utf8FromUtf16(argv[i]));
  }

  ::LocalFree(argv);

  return command_line_arguments;
}

std::string Utf8FromUtf16(const wchar_t* utf16_string) {
  if (utf16_string == nullptr) {
    return std::string();
  }
  // First, find the length of the string with a safe upper bound (CWE-126).
  // UNICODE_STRING_MAX_CHARS (32767) is the maximum length of a UNICODE_STRING.
  int input_length = static_cast<int>(wcsnlen(utf16_string, UNICODE_STRING_MAX_CHARS));
  // Now use that bounded length to determine the required buffer size.
  // When an explicit length is passed, WideCharToMultiByte does not include
  // the null terminator in its returned size.
  int target_length = ::WideCharToMultiByte(
      CP_UTF8, WC_ERR_INVALID_CHARS, utf16_string,
      input_length, nullptr, 0, nullptr, nullptr);
  std::string utf8_string;
  if (target_length == 0 || static_cast<size_t>(target_length) > utf8_string.max_size()) {
    return utf8_string;
  }
  utf8_string.resize(target_length);
  int converted_length = ::WideCharToMultiByte(
      CP_UTF8, WC_ERR_INVALID_CHARS, utf16_string,
      input_length, utf8_string.data(), target_length, nullptr, nullptr);
  if (converted_length == 0) {
    return std::string();
  }
  return utf8_string;
}

std::wstring Utf16FromUtf8(const std::string& utf8_string) {
  if (utf8_string.empty()) {
    return std::wstring();
  }
  if (utf8_string.size() > static_cast<size_t>(INT_MAX)) {
    return std::wstring();
  }
  const int input_length = static_cast<int>(utf8_string.size());
  int target_length =
      ::MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, utf8_string.data(),
                            input_length, nullptr, 0);
  if (target_length <= 0) {
    return std::wstring();
  }
  std::wstring utf16_string(static_cast<size_t>(target_length), L'\0');
  int converted_length = ::MultiByteToWideChar(
      CP_UTF8, MB_ERR_INVALID_CHARS, utf8_string.data(), input_length,
      utf16_string.data(), target_length);
  if (converted_length != target_length) {
    return std::wstring();
  }
  return utf16_string;
}

std::string Win32ErrorMessage(unsigned long code) {
  wchar_t* buffer = nullptr;
  DWORD length = ::FormatMessageW(
      FORMAT_MESSAGE_ALLOCATE_BUFFER | FORMAT_MESSAGE_FROM_SYSTEM |
          FORMAT_MESSAGE_IGNORE_INSERTS,
      nullptr, code, 0, reinterpret_cast<wchar_t*>(&buffer), 0, nullptr);
  std::string message;
  if (length > 0 && buffer != nullptr) {
    message = Utf8FromUtf16(buffer);
    // FormatMessage ends its text with a line break.
    while (!message.empty() &&
           (message.back() == '\n' || message.back() == '\r' ||
            message.back() == ' ')) {
      message.pop_back();
    }
  }
  if (buffer != nullptr) {
    ::LocalFree(buffer);
  }
  if (message.empty()) {
    message = "Error";
  }
  return message + " (" + std::to_string(code) + ")";
}

void Log(const std::string& message) {
  std::string line = "Claktory: " + message + "\n";
  ::OutputDebugStringW(Utf16FromUtf8(line).c_str());
  std::fputs(line.c_str(), stderr);
  std::fflush(stderr);
  // CLAKTORY_LOG_FILE: also appended to that file, for test runs that start
  // the app with no console or debugger to read it (Hermes's runner jobs).
  // Read once; its lines are the runner's log only, never a secret.
  static const std::wstring log_file = [] {
    wchar_t path[MAX_PATH * 4];
    DWORD length = ::GetEnvironmentVariableW(L"CLAKTORY_LOG_FILE", path,
                                             static_cast<DWORD>(std::size(path)));
    return length > 0 && length < std::size(path) ? std::wstring(path, length)
                                                  : std::wstring();
  }();
  if (log_file.empty()) return;
  HANDLE file = ::CreateFileW(log_file.c_str(), FILE_APPEND_DATA,
                              FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr,
                              OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
  if (file == INVALID_HANDLE_VALUE) return;
  DWORD written = 0;
  ::WriteFile(file, line.data(), static_cast<DWORD>(line.size()), &written,
              nullptr);
  ::CloseHandle(file);
}
