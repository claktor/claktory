#ifndef RUNNER_UTILS_H_
#define RUNNER_UTILS_H_

#include <string>
#include <vector>

// Creates a console for the process, and redirects stdout and stderr to
// it for both the runner and the Flutter library.
void CreateAndAttachConsole();

// Takes a null-terminated wchar_t* encoded in UTF-16 and returns a std::string
// encoded in UTF-8. Returns an empty std::string on failure.
std::string Utf8FromUtf16(const wchar_t* utf16_string);

// Takes UTF-8 and returns it as UTF-16; an empty std::wstring when it isn't
// valid UTF-8.
std::wstring Utf16FromUtf8(const std::string& utf8_string);

// The system's words for a Win32 error code (GetLastError), in UTF-8, with its
// number: "Access is denied. (5)".
std::string Win32ErrorMessage(unsigned long code);

// Writes "Claktory: <message>" to the debugger's output (OutputDebugString),
// to stderr, which `flutter run` shows, and, when CLAKTORY_LOG_FILE names a
// file, to the end of that file: the runner's log, as the Mac's NSLog.
void Log(const std::string& message);

// Gets the command line arguments passed in as a std::vector<std::string>,
// encoded in UTF-8. Returns an empty std::vector<std::string> on failure.
std::vector<std::string> GetCommandLineArguments();

#endif  // RUNNER_UTILS_H_
