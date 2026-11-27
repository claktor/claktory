#include "file_dialogs.h"

#include <shobjidl.h>

#include "utils.h"

namespace {

// A COM pointer released when it goes, without pulling in ATL or WRL.
template <typename T>
class ComPtr {
 public:
  ComPtr() = default;
  ~ComPtr() {
    if (pointer_ != nullptr) pointer_->Release();
  }
  ComPtr(const ComPtr&) = delete;
  ComPtr& operator=(const ComPtr&) = delete;

  T** put() { return &pointer_; }
  T* get() const { return pointer_; }
  T* operator->() const { return pointer_; }
  explicit operator bool() const { return pointer_ != nullptr; }

 private:
  T* pointer_ = nullptr;
};

std::optional<std::string> PathOf(IShellItem* item) {
  PWSTR path = nullptr;
  if (FAILED(item->GetDisplayName(SIGDN_FILESYSPATH, &path)) ||
      path == nullptr) {
    return std::nullopt;
  }
  std::string utf8 = Utf8FromUtf16(path);
  ::CoTaskMemFree(path);
  if (utf8.empty()) return std::nullopt;
  return utf8;
}

bool MakeDialog(ComPtr<IFileOpenDialog>* dialog) {
  HRESULT made =
      ::CoCreateInstance(CLSID_FileOpenDialog, nullptr, CLSCTX_INPROC_SERVER,
                         IID_PPV_ARGS(dialog->put()));
  if (FAILED(made)) {
    Log("couldn't open the Open dialog: " +
        Win32ErrorMessage(static_cast<unsigned long>(made)));
    return false;
  }
  return true;
}

}  // namespace

std::optional<std::string> PickFolder(HWND owner, const std::string& from) {
  ComPtr<IFileOpenDialog> dialog;
  if (!MakeDialog(&dialog)) return std::nullopt;
  FILEOPENDIALOGOPTIONS options = 0;
  dialog->GetOptions(&options);
  dialog->SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM |
                     FOS_PATHMUSTEXIST | FOS_NOCHANGEDIR);
  dialog->SetOkButtonLabel(L"Choose");
  if (!from.empty()) {
    std::wstring start = Utf16FromUtf8(from);
    DWORD attributes = ::GetFileAttributesW(start.c_str());
    if (!start.empty() && attributes != INVALID_FILE_ATTRIBUTES &&
        (attributes & FILE_ATTRIBUTE_DIRECTORY) != 0) {
      ComPtr<IShellItem> folder;
      if (SUCCEEDED(::SHCreateItemFromParsingName(start.c_str(), nullptr,
                                                  IID_PPV_ARGS(folder.put())))) {
        dialog->SetFolder(folder.get());
      }
    }
  }
  if (FAILED(dialog->Show(owner))) return std::nullopt;  // Cancelled.
  ComPtr<IShellItem> chosen;
  if (FAILED(dialog->GetResult(chosen.put())) || !chosen) return std::nullopt;
  return PathOf(chosen.get());
}

std::vector<std::string> PickFiles(HWND owner) {
  std::vector<std::string> paths;
  ComPtr<IFileOpenDialog> dialog;
  if (!MakeDialog(&dialog)) return paths;
  FILEOPENDIALOGOPTIONS options = 0;
  dialog->GetOptions(&options);
  dialog->SetOptions(options | FOS_ALLOWMULTISELECT | FOS_FORCEFILESYSTEM |
                     FOS_FILEMUSTEXIST | FOS_PATHMUSTEXIST | FOS_NOCHANGEDIR);
  dialog->SetOkButtonLabel(L"Attach");
  if (FAILED(dialog->Show(owner))) return paths;  // Cancelled.
  ComPtr<IShellItemArray> chosen;
  if (FAILED(dialog->GetResults(chosen.put())) || !chosen) return paths;
  DWORD count = 0;
  if (FAILED(chosen->GetCount(&count))) return paths;
  for (DWORD i = 0; i < count; ++i) {
    ComPtr<IShellItem> item;
    if (FAILED(chosen->GetItemAt(i, item.put()))) continue;
    if (std::optional<std::string> path = PathOf(item.get())) {
      paths.push_back(std::move(*path));
    }
  }
  return paths;
}
