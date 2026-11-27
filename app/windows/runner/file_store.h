#ifndef RUNNER_FILE_STORE_H_
#define RUNNER_FILE_STORE_H_

#include <optional>
#include <string>

#include "core/connection_store.h"
#include "core/json.h"

// Files the runner keeps in the app's folder (app_paths.h), each written
// whole or not at all: a new copy beside it, flushed, then moved over the old
// one in one step (MoveFileEx with MOVEFILE_REPLACE_EXISTING |
// MOVEFILE_WRITE_THROUGH), so a crash or a power cut leaves the old one or the
// new one, never half of either.

struct FileRead {
  enum class Status { kFound, kMissing, kFailed };
  Status status = Status::kMissing;
  std::string data;
  std::string error;
};

FileRead ReadWholeFile(const std::wstring& path);

// An error's words, or nullopt once [data] is in [path].
std::optional<std::string> WriteWholeFile(const std::wstring& path,
                                          const std::string& data);

// Removes [path]; nothing there is no failure.
std::optional<std::string> DeleteFileIfThere(const std::wstring& path);

// A JSON file in the app's folder: the drafts, the window's place. Plain JSON:
// the folder is this account's only, as the Mac's user defaults are.
class JsonFile {
 public:
  explicit JsonFile(const wchar_t* name) : name_(name) {}

  // What's kept; nullopt when nothing is, or it can't be read as JSON (logged).
  std::optional<claktory::Json> Read() const;
  // Logs and answers the error's words when it can't be kept.
  std::optional<std::string> Write(const claktory::Json& value) const;

 private:
  const wchar_t* name_;
};

// The connections' blob (core/connection_store.h): one file in the app's
// folder, connections.dpapi, holding a short header and what
// CryptProtectData made of the JSON, with user DPAPI: only this Windows account
// on this PC opens it, and nothing asks the person anything
// (CRYPTPROTECT_UI_FORBIDDEN). Any program running as this account can open
// it too (windows.md section 3: DPAPI protects against other accounts and an
// offline disk, not this account's own processes). No plain-text copy is ever
// written, and one that can't be opened is never replaced by itself.
class DpapiFile : public claktory::SecretBlob {
 public:
  explicit DpapiFile(const wchar_t* name) : name_(name) {}

  ReadResult Read() override;
  std::optional<std::string> Write(const std::string& data) override;
  std::optional<std::string> Erase() override;

 private:
  const wchar_t* name_;
};

#endif  // RUNNER_FILE_STORE_H_
