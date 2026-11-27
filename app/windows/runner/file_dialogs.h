#ifndef RUNNER_FILE_DIALOGS_H_
#define RUNNER_FILE_DIALOGS_H_

#include <windows.h>

#include <optional>
#include <string>
#include <vector>

// The system's Open dialog (IFileOpenDialog), as the Mac's NSOpenPanel sheets
// in MainFlutterWindow.swift: modal on [owner], paths in UTF-8.

// A folder, starting in [from] when it's there; its path, or nullopt when the
// person cancels. Its button says Choose, as the Mac's.
std::optional<std::string> PickFolder(HWND owner, const std::string& from);

// Files to attach; their paths, none when the person cancels. Its button says
// Attach, as the Mac's.
std::vector<std::string> PickFiles(HWND owner);

#endif  // RUNNER_FILE_DIALOGS_H_
