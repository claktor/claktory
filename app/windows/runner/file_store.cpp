#include "file_store.h"

#include <windows.h>
#include <wincrypt.h>

#include <cstring>

#include "app_paths.h"
#include "utils.h"

namespace {

// Nothing the runner keeps is anywhere near this; a bigger file isn't ours.
constexpr LONGLONG kMaxFileSize = 16 * 1024 * 1024;

// The start of connections.dpapi, then the DPAPI blob: says what the file is
// and lets a later format be told apart.
constexpr char kDpapiHeader[] = "CLAKTORY-DPAPI-1\n";
constexpr size_t kDpapiHeaderSize = sizeof(kDpapiHeader) - 1;

// DPAPI's optional entropy: a blob made for another purpose (or another
// program using DPAPI the plain way) doesn't open as this one. It's no secret
// from programs running as this account, which can read it here.
constexpr char kDpapiEntropy[] = "Claktory connections v1";

std::string Utf8Path(const std::wstring& path) {
  return Utf8FromUtf16(path.c_str());
}

}  // namespace

FileRead ReadWholeFile(const std::wstring& path) {
  FileRead result;
  HANDLE file = ::CreateFileW(path.c_str(), GENERIC_READ,
                              FILE_SHARE_READ | FILE_SHARE_DELETE, nullptr,
                              OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
  if (file == INVALID_HANDLE_VALUE) {
    DWORD error = ::GetLastError();
    if (error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND) {
      result.status = FileRead::Status::kMissing;
    } else {
      result.status = FileRead::Status::kFailed;
      result.error = Win32ErrorMessage(error);
    }
    return result;
  }
  LARGE_INTEGER size{};
  if (!::GetFileSizeEx(file, &size)) {
    result.status = FileRead::Status::kFailed;
    result.error = Win32ErrorMessage(::GetLastError());
    ::CloseHandle(file);
    return result;
  }
  if (size.QuadPart > kMaxFileSize) {
    result.status = FileRead::Status::kFailed;
    result.error = "The file is too big to be Claktory's (" +
                   std::to_string(size.QuadPart) + " bytes).";
    ::CloseHandle(file);
    return result;
  }
  result.data.resize(static_cast<size_t>(size.QuadPart));
  size_t done = 0;
  while (done < result.data.size()) {
    DWORD read = 0;
    DWORD want = static_cast<DWORD>(result.data.size() - done);
    if (!::ReadFile(file, &result.data[done], want, &read, nullptr)) {
      result.status = FileRead::Status::kFailed;
      result.error = Win32ErrorMessage(::GetLastError());
      result.data.clear();
      ::CloseHandle(file);
      return result;
    }
    if (read == 0) break;  // It got shorter meanwhile.
    done += read;
  }
  result.data.resize(done);
  ::CloseHandle(file);
  result.status = FileRead::Status::kFound;
  return result;
}

std::optional<std::string> WriteWholeFile(const std::wstring& path,
                                          const std::string& data) {
  static unsigned int counter = 0;
  std::wstring temporary = path + L"." + std::to_wstring(::GetCurrentProcessId()) +
                           L"-" + std::to_wstring(++counter) + L".new";
  // Made with the folder's DACL (app_paths.cpp): this account and SYSTEM.
  HANDLE file =
      ::CreateFileW(temporary.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS,
                    FILE_ATTRIBUTE_NORMAL, nullptr);
  if (file == INVALID_HANDLE_VALUE) {
    return Win32ErrorMessage(::GetLastError());
  }
  size_t done = 0;
  while (done < data.size()) {
    DWORD written = 0;
    DWORD want = static_cast<DWORD>(data.size() - done);
    if (!::WriteFile(file, data.data() + done, want, &written, nullptr) ||
        written == 0) {
      DWORD error = ::GetLastError();
      ::CloseHandle(file);
      ::DeleteFileW(temporary.c_str());
      return Win32ErrorMessage(error);
    }
    done += written;
  }
  if (!::FlushFileBuffers(file)) {
    DWORD error = ::GetLastError();
    ::CloseHandle(file);
    ::DeleteFileW(temporary.c_str());
    return Win32ErrorMessage(error);
  }
  ::CloseHandle(file);
  // An antivirus scanning the old file can hold it for a moment.
  DWORD error = ERROR_SUCCESS;
  for (int attempt = 0; attempt < 5; ++attempt) {
    if (::MoveFileExW(temporary.c_str(), path.c_str(),
                      MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)) {
      return std::nullopt;
    }
    error = ::GetLastError();
    if (error != ERROR_SHARING_VIOLATION && error != ERROR_ACCESS_DENIED &&
        error != ERROR_LOCK_VIOLATION) {
      break;
    }
    ::Sleep(20 * (attempt + 1));
  }
  ::DeleteFileW(temporary.c_str());
  return Win32ErrorMessage(error);
}

std::optional<std::string> DeleteFileIfThere(const std::wstring& path) {
  if (::DeleteFileW(path.c_str())) return std::nullopt;
  DWORD error = ::GetLastError();
  if (error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND) {
    return std::nullopt;
  }
  return Win32ErrorMessage(error);
}

std::optional<claktory::Json> JsonFile::Read() const {
  std::optional<std::wstring> path = AppDataFile(name_);
  if (!path) return std::nullopt;
  FileRead kept = ReadWholeFile(*path);
  if (kept.status == FileRead::Status::kMissing) return std::nullopt;
  if (kept.status == FileRead::Status::kFailed) {
    Log("couldn't read " + Utf8Path(*path) + ": " + kept.error);
    return std::nullopt;
  }
  std::string error;
  std::optional<claktory::Json> value = claktory::ReadJson(kept.data, &error);
  if (!value) {
    Log(Utf8Path(*path) + " isn't JSON (" + error + "), so it's left unread");
  }
  return value;
}

std::optional<std::string> JsonFile::Write(const claktory::Json& value) const {
  std::optional<std::wstring> path = AppDataFile(name_);
  if (!path) return std::string("Claktory's folder for this app is missing.");
  std::optional<std::string> error =
      WriteWholeFile(*path, claktory::WriteJson(value));
  if (error) Log("couldn't write " + Utf8Path(*path) + ": " + *error);
  return error;
}

DpapiFile::ReadResult DpapiFile::Read() {
  ReadResult result;
  std::optional<std::wstring> path = AppDataFile(name_);
  if (!path) {
    result.status = ReadResult::Status::kUnavailable;
    result.error = "Claktory's folder for this app is missing.";
    return result;
  }
  FileRead kept = ReadWholeFile(*path);
  switch (kept.status) {
    case FileRead::Status::kMissing:
      result.status = ReadResult::Status::kMissing;
      return result;
    case FileRead::Status::kFailed:
      result.status = ReadResult::Status::kUnavailable;
      result.error = kept.error;
      return result;
    case FileRead::Status::kFound:
      break;
  }
  if (kept.data.size() <= kDpapiHeaderSize ||
      kept.data.compare(0, kDpapiHeaderSize, kDpapiHeader) != 0) {
    result.status = ReadResult::Status::kUnreadable;
    result.error = "The file isn't Claktory's connections.";
    return result;
  }
  DATA_BLOB sealed{};
  sealed.cbData = static_cast<DWORD>(kept.data.size() - kDpapiHeaderSize);
  sealed.pbData = reinterpret_cast<BYTE*>(&kept.data[kDpapiHeaderSize]);
  DATA_BLOB entropy{};
  entropy.cbData = static_cast<DWORD>(sizeof(kDpapiEntropy) - 1);
  entropy.pbData = reinterpret_cast<BYTE*>(const_cast<char*>(kDpapiEntropy));
  DATA_BLOB opened{};
  if (!::CryptUnprotectData(&sealed, nullptr, &entropy, nullptr, nullptr,
                            CRYPTPROTECT_UI_FORBIDDEN, &opened)) {
    DWORD error = ::GetLastError();
    // Not this account's, damaged, or its key gone (the account's password
    // reset by an administrator): it won't open by trying again.
    bool for_good = error == ERROR_INVALID_DATA ||
                    error == static_cast<DWORD>(NTE_BAD_DATA) ||
                    error == static_cast<DWORD>(NTE_BAD_KEY_STATE) ||
                    error == static_cast<DWORD>(NTE_BAD_KEY);
    result.status = for_good ? ReadResult::Status::kUnreadable
                             : ReadResult::Status::kUnavailable;
    result.error = Win32ErrorMessage(error);
    return result;
  }
  result.data.assign(reinterpret_cast<const char*>(opened.pbData),
                     opened.cbData);
  ::SecureZeroMemory(opened.pbData, opened.cbData);
  ::LocalFree(opened.pbData);
  result.status = ReadResult::Status::kFound;
  return result;
}

std::optional<std::string> DpapiFile::Write(const std::string& data) {
  std::optional<std::wstring> path = AppDataFile(name_);
  if (!path) return std::string("Claktory's folder for this app is missing.");
  DATA_BLOB plain{};
  plain.cbData = static_cast<DWORD>(data.size());
  plain.pbData = reinterpret_cast<BYTE*>(const_cast<char*>(data.data()));
  DATA_BLOB entropy{};
  entropy.cbData = static_cast<DWORD>(sizeof(kDpapiEntropy) - 1);
  entropy.pbData = reinterpret_cast<BYTE*>(const_cast<char*>(kDpapiEntropy));
  DATA_BLOB sealed{};
  if (!::CryptProtectData(&plain, L"Claktory connections", &entropy, nullptr,
                          nullptr, CRYPTPROTECT_UI_FORBIDDEN, &sealed)) {
    // Access denied here is a sign-in without the account's password (ssh
    // with a key, a scheduled task without one): DPAPI can't open its key.
    return "Windows couldn't protect them: " +
           Win32ErrorMessage(::GetLastError());
  }
  std::string file(kDpapiHeader, kDpapiHeaderSize);
  file.append(reinterpret_cast<const char*>(sealed.pbData), sealed.cbData);
  ::LocalFree(sealed.pbData);
  return WriteWholeFile(*path, file);
}

std::optional<std::string> DpapiFile::Erase() {
  std::optional<std::wstring> path = AppDataFile(name_);
  if (!path) return std::nullopt;  // No folder: nothing kept.
  return DeleteFileIfThere(*path);
}
