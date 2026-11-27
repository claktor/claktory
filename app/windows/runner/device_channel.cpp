#include "device_channel.h"

#include <flutter/standard_method_codec.h>
#include <windows.h>
#include <knownfolders.h>
#include <shlobj.h>

#include <string>
#include <variant>
#include <vector>

#include "app_paths.h"
#include "encodable_json.h"
#include "file_store.h"
#include "utils.h"

using claktory::Json;
using claktory::StoreFailure;
using flutter::EncodableMap;
using flutter::EncodableValue;

namespace {

constexpr wchar_t kConnectionsFile[] = L"connections.dpapi";
constexpr wchar_t kDraftsFile[] = L"drafts.json";

// The person's Downloads folder, wherever they moved it; empty if Windows
// can't say.
std::string DownloadsFolder() {
  PWSTR path = nullptr;
  std::string folder;
  if (SUCCEEDED(::SHGetKnownFolderPath(FOLDERID_Downloads, KF_FLAG_DEFAULT,
                                       nullptr, &path)) &&
      path != nullptr) {
    folder = Utf8FromUtf16(path);
  }
  if (path != nullptr) ::CoTaskMemFree(path);
  return folder;
}

// Opens Explorer on the folder holding [paths] (all in one folder), with
// them selected, as the Mac's Show in Finder; false when it can't.
bool ShowInExplorer(const std::vector<std::string>& paths) {
  if (paths.empty()) return false;
  std::wstring first = Utf16FromUtf8(paths.front());
  size_t slash = first.find_last_of(L"\\/");
  if (first.empty() || slash == std::wstring::npos) return false;
  std::wstring folder_path = first.substr(0, slash);
  PIDLIST_ABSOLUTE folder = ::ILCreateFromPathW(folder_path.c_str());
  if (folder == nullptr) return false;
  std::vector<PIDLIST_ABSOLUTE> items;
  // Full paths' items stand for children of the folder, as Microsoft
  // documents for this call.
  std::vector<PCUITEMID_CHILD> children;
  for (const auto& path : paths) {
    std::wstring wide = Utf16FromUtf8(path);
    if (wide.empty()) continue;
    if (PIDLIST_ABSOLUTE item = ::ILCreateFromPathW(wide.c_str())) {
      items.push_back(item);
      children.push_back(reinterpret_cast<PCUITEMID_CHILD>(item));
    }
  }
  HRESULT shown = ::SHOpenFolderAndSelectItems(
      folder, static_cast<UINT>(children.size()),
      children.empty() ? nullptr : children.data(), 0);
  for (auto item : items) ::ILFree(item);
  ::ILFree(folder);
  return SUCCEEDED(shown);
}

// This PC's Claktory's app-token (windows.md section 3, B): 64 hex
// characters its backend writes at every start into [path]
// (<home>\app-token), with nothing around them but whitespace. Opened
// sharing delete, so a read never stops the backend replacing it (Fugo
// 48393); read whole, never logged. Empty with [missing] set when there's
// no file; empty otherwise when it can't be read or isn't a token.
std::string ReadAppToken(const std::wstring& path, bool* missing,
                         std::string* error) {
  *missing = false;
  HANDLE file = ::CreateFileW(
      path.c_str(), GENERIC_READ,
      FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
      OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
  if (file == INVALID_HANDLE_VALUE) {
    DWORD code = ::GetLastError();
    *missing = code == ERROR_FILE_NOT_FOUND || code == ERROR_PATH_NOT_FOUND;
    *error = Win32ErrorMessage(code);
    return std::string();
  }
  char buffer[256];
  DWORD read = 0;
  BOOL done = ::ReadFile(file, buffer, sizeof(buffer), &read, nullptr);
  DWORD code = ::GetLastError();
  ::CloseHandle(file);
  if (!done) {
    *error = Win32ErrorMessage(code);
    ::SecureZeroMemory(buffer, sizeof(buffer));
    return std::string();
  }
  std::string token;
  bool bad = read == sizeof(buffer);
  for (DWORD i = 0; i < read && !bad; ++i) {
    char c = buffer[i];
    if (c == ' ' || c == '\t' || c == '\r' || c == '\n') {
      // Whitespace only around the token, never inside it.
      if (!token.empty()) {
        for (DWORD j = i; j < read; ++j) {
          char after = buffer[j];
          if (after != ' ' && after != '\t' && after != '\r' && after != '\n') {
            bad = true;
          }
        }
        break;
      }
      continue;
    }
    bool hex = (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') ||
               (c >= 'A' && c <= 'F');
    if (!hex) bad = true;
    token += c;
  }
  ::SecureZeroMemory(buffer, sizeof(buffer));
  if (bad || token.size() != 64) {
    ::SecureZeroMemory(token.data(), token.size());
    *error = "The file isn't a Claktory app-token.";
    return std::string();
  }
  return token;
}

}  // namespace

DeviceChannel::DeviceChannel(flutter::BinaryMessenger* messenger)
    : channel_(std::make_unique<flutter::MethodChannel<EncodableValue>>(
          messenger, "factory/device",
          &flutter::StandardMethodCodec::GetInstance())),
      connections_(std::make_unique<claktory::ConnectionStore>(
          std::make_unique<DpapiFile>(kConnectionsFile),
          [](const std::string& message) { Log("connections: " + message); })) {
  channel_->SetMethodCallHandler(
      [this](const auto& call, auto result) { Handle(call, std::move(result)); });
}

void DeviceChannel::Handle(
    const flutter::MethodCall<EncodableValue>& call,
    std::unique_ptr<flutter::MethodResult<EncodableValue>> result) {
  const std::string& method = call.method_name();
  const EncodableValue* arguments = call.arguments();

  if (method == "connections") {
    std::variant<Json, StoreFailure> kept = connections_->Read();
    if (auto* failure = std::get_if<StoreFailure>(&kept)) {
      result->Error(failure->code, failure->message);
      return;
    }
    result->Success(EncodableFromJson(std::get<Json>(kept)));
    return;
  }

  if (method == "setConnections") {
    // A map, as connections.dart sends; anything else is refused rather
    // than kept as no connections.
    std::string why = "they aren't a map";
    std::optional<Json> connections;
    if (arguments != nullptr && std::holds_alternative<EncodableMap>(*arguments)) {
      connections = JsonFromEncodable(*arguments, &why);
    }
    if (!connections) {
      result->Error("credentials_write_failed", "Not kept: " + why + ".");
      return;
    }
    if (std::optional<StoreFailure> failure = connections_->Write(*connections)) {
      result->Error(failure->code, failure->message);
      return;
    }
    result->Success();
    return;
  }

  if (method == "resetConnections") {
    // Only when the person chooses Start over.
    if (std::optional<StoreFailure> failure = connections_->Reset()) {
      result->Error(failure->code, failure->message);
      return;
    }
    result->Success();
    return;
  }

  if (method == "drafts") {
    EncodableMap drafts;
    if (std::optional<Json> kept = JsonFile(kDraftsFile).Read()) {
      for (const auto& entry : kept->map()) {
        if (entry.second.is_string()) {
          drafts[EncodableValue(entry.first)] =
              EncodableValue(entry.second.string_value());
        }
      }
    }
    result->Success(EncodableValue(std::move(drafts)));
    return;
  }

  if (method == "setDrafts") {
    Json drafts = Json::MakeMap();
    if (arguments != nullptr) {
      if (const auto* map = std::get_if<EncodableMap>(arguments)) {
        for (const auto& entry : *map) {
          const auto* key = std::get_if<std::string>(&entry.first);
          const auto* text = std::get_if<std::string>(&entry.second);
          if (key != nullptr && text != nullptr) {
            drafts.Set(*key, Json::String(*text));
          }
        }
      }
    }
    // As the Mac's user defaults: a draft that can't be kept is logged, and
    // the message box keeps it meanwhile.
    JsonFile(kDraftsFile).Write(drafts);
    result->Success();
    return;
  }

  if (method == "cacheFolder") {
    // Where copies of files the viewers read are kept (lib/viewers/
    // file_cache.dart): inside the app's private folder, so they're this
    // account's only, as the files they copy are the person's.
    std::optional<std::wstring> folder = AppDataFolder();
    if (!folder) {
      result->Success();
      return;
    }
    std::wstring cache = *folder + L"\\Cache";
    ::CreateDirectoryW(cache.c_str(), nullptr);  // Inherits the folder's DACL.
    result->Success(EncodableValue(Utf8FromUtf16(cache.c_str())));
    return;
  }

  if (method == "downloadsFolder") {
    std::string folder = DownloadsFolder();
    if (folder.empty()) {
      result->Success();
    } else {
      result->Success(EncodableValue(folder));
    }
    return;
  }

  if (method == "appToken") {
    const auto* path =
        arguments != nullptr ? std::get_if<std::string>(arguments) : nullptr;
    if (path == nullptr || path->empty()) {
      result->Error("bad_arguments", "appToken takes the file's path.");
      return;
    }
    bool missing = false;
    std::string error;
    std::string token = ReadAppToken(Utf16FromUtf8(*path), &missing, &error);
    if (!token.empty()) {
      result->Success(EncodableValue(token));
      ::SecureZeroMemory(token.data(), token.size());
    } else if (missing) {
      result->Success();
    } else {
      result->Error("app_token_unreadable", error);
    }
    return;
  }

  if (method == "showFiles") {
    std::vector<std::string> paths;
    if (arguments != nullptr) {
      if (const auto* list = std::get_if<flutter::EncodableList>(arguments)) {
        for (const auto& item : *list) {
          if (const auto* path = std::get_if<std::string>(&item)) {
            paths.push_back(*path);
          }
        }
      }
    }
    result->Success(EncodableValue(ShowInExplorer(paths)));
    return;
  }

  result->NotImplemented();
}
