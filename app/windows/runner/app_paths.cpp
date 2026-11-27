#include "app_paths.h"

#include <windows.h>
#include <aclapi.h>
#include <knownfolders.h>
#include <sddl.h>
#include <shlobj.h>

#include <memory>

#include "flavor.h"
#include "utils.h"

namespace {

// The account this process runs as: its SID, binary and as text.
struct UserSid {
  std::unique_ptr<BYTE[]> token_user;  // A TOKEN_USER; Sid points inside.
  PSID sid = nullptr;
  std::wstring text;
};

std::optional<UserSid> CurrentUser() {
  HANDLE token = nullptr;
  if (!::OpenProcessToken(::GetCurrentProcess(), TOKEN_QUERY, &token)) {
    return std::nullopt;
  }
  DWORD size = 0;
  ::GetTokenInformation(token, TokenUser, nullptr, 0, &size);
  UserSid user;
  user.token_user.reset(new BYTE[size > 0 ? size : 1]);
  bool found = size > 0 && ::GetTokenInformation(token, TokenUser,
                                                 user.token_user.get(), size,
                                                 &size);
  ::CloseHandle(token);
  if (!found) return std::nullopt;
  user.sid = reinterpret_cast<TOKEN_USER*>(user.token_user.get())->User.Sid;
  wchar_t* text = nullptr;
  if (!::ConvertSidToStringSidW(user.sid, &text)) return std::nullopt;
  user.text = text;
  ::LocalFree(text);
  return user;
}

// A security descriptor only this account and SYSTEM can open: owned by the
// account; P: protected (nothing inherited from above); OICI: what's inside
// inherits it; FA: full access; SY: SYSTEM. No Administrators: an elevated
// token of this same account still matches its SID. (Administrators can take
// ownership anyway, so it's no protection from them, only from other
// accounts' ordinary access.)
class PrivateDescriptor {
 public:
  explicit PrivateDescriptor(const UserSid& user) {
    std::wstring sddl = L"O:" + user.text + L"D:P(A;OICI;FA;;;" + user.text +
                        L")(A;OICI;FA;;;SY)";
    if (!::ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.c_str(), SDDL_REVISION_1, &descriptor_, nullptr)) {
      error_ = ::GetLastError();
      descriptor_ = nullptr;
    }
  }
  ~PrivateDescriptor() {
    if (descriptor_ != nullptr) ::LocalFree(descriptor_);
  }
  PrivateDescriptor(const PrivateDescriptor&) = delete;
  PrivateDescriptor& operator=(const PrivateDescriptor&) = delete;

  PSECURITY_DESCRIPTOR get() const { return descriptor_; }
  DWORD error() const { return error_; }

  PACL Dacl() const {
    BOOL present = FALSE;
    BOOL defaulted = FALSE;
    PACL dacl = nullptr;
    if (descriptor_ == nullptr ||
        !::GetSecurityDescriptorDacl(descriptor_, &present, &dacl,
                                     &defaulted) ||
        !present) {
      return nullptr;
    }
    return dacl;
  }

 private:
  PSECURITY_DESCRIPTOR descriptor_ = nullptr;
  DWORD error_ = ERROR_SUCCESS;
};

// An app folder already there (from an older build, or made under a
// redirected %LOCALAPPDATA%) is taken only if it's a real folder, not a
// junction or link, owned by this account; its DACL is then made the private
// one again (Fugo 45231).
bool TakeExistingFolder(const std::wstring& path, DWORD attributes,
                        const UserSid& user,
                        const PrivateDescriptor& descriptor) {
  std::string name = Utf8FromUtf16(path.c_str());
  if ((attributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0 ||
      (attributes & FILE_ATTRIBUTE_DIRECTORY) == 0) {
    Log(name + " isn't a plain folder (a link or a file), so it isn't used");
    return false;
  }
  PSID owner = nullptr;
  PSECURITY_DESCRIPTOR existing = nullptr;
  DWORD read = ::GetNamedSecurityInfoW(path.c_str(), SE_FILE_OBJECT,
                                       OWNER_SECURITY_INFORMATION, &owner,
                                       nullptr, nullptr, nullptr, &existing);
  if (read != ERROR_SUCCESS) {
    Log("couldn't read who owns " + name + ": " + Win32ErrorMessage(read));
    return false;
  }
  bool ours = owner != nullptr && ::EqualSid(owner, user.sid);
  ::LocalFree(existing);
  if (!ours) {
    Log(name + " belongs to another account, so it isn't used");
    return false;
  }
  PACL dacl = descriptor.Dacl();
  if (dacl == nullptr) {
    Log("couldn't make the app folder's DACL");
    return false;
  }
  DWORD set = ::SetNamedSecurityInfoW(
      const_cast<wchar_t*>(path.c_str()), SE_FILE_OBJECT,
      DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, nullptr,
      nullptr, dacl, nullptr);
  if (set != ERROR_SUCCESS) {
    Log("couldn't make " + name + " private: " + Win32ErrorMessage(set));
    return false;
  }
  return true;
}

// Makes [path] a folder only this account and SYSTEM can open, or takes one
// already there that can be made so; true when it's ready.
bool MakePrivateFolder(const std::wstring& path) {
  std::optional<UserSid> user = CurrentUser();
  if (!user) {
    Log("couldn't learn this account's SID");
    return false;
  }
  PrivateDescriptor descriptor(*user);
  if (descriptor.get() == nullptr) {
    Log("couldn't make the app folder's DACL: " +
        Win32ErrorMessage(descriptor.error()));
    return false;
  }
  DWORD attributes = ::GetFileAttributesW(path.c_str());
  if (attributes != INVALID_FILE_ATTRIBUTES) {
    return TakeExistingFolder(path, attributes, *user, descriptor);
  }
  SECURITY_ATTRIBUTES security{};
  security.nLength = sizeof(security);
  security.lpSecurityDescriptor = descriptor.get();
  security.bInheritHandle = FALSE;
  if (!::CreateDirectoryW(path.c_str(), &security)) {
    DWORD error = ::GetLastError();
    if (error == ERROR_ALREADY_EXISTS) {
      // Made meanwhile (another copy of the app starting): checked as any.
      attributes = ::GetFileAttributesW(path.c_str());
      return attributes != INVALID_FILE_ATTRIBUTES &&
             TakeExistingFolder(path, attributes, *user, descriptor);
    }
    Log("couldn't make " + Utf8FromUtf16(path.c_str()) + ": " +
        Win32ErrorMessage(error));
    return false;
  }
  return true;
}

std::optional<std::wstring> FindFolder() {
  PWSTR local = nullptr;
  HRESULT found = ::SHGetKnownFolderPath(FOLDERID_LocalAppData, KF_FLAG_CREATE,
                                         nullptr, &local);
  if (FAILED(found) || local == nullptr) {
    if (local != nullptr) ::CoTaskMemFree(local);
    Log("couldn't find %LOCALAPPDATA%");
    return std::nullopt;
  }
  std::wstring claktory = std::wstring(local) + L"\\Claktory";
  ::CoTaskMemFree(local);
  // Claktory\ itself is shared with the backend's data folder (factory\), so
  // it's made with what %LOCALAPPDATA% gives (already this account's only);
  // the app's own folder inside is made private.
  if (!::CreateDirectoryW(claktory.c_str(), nullptr)) {
    DWORD error = ::GetLastError();
    if (error != ERROR_ALREADY_EXISTS) {
      Log("couldn't make " + Utf8FromUtf16(claktory.c_str()) + ": " +
          Win32ErrorMessage(error));
      return std::nullopt;
    }
  }
  std::wstring folder = claktory + L"\\" + flavor::kDataFolder;
  if (!MakePrivateFolder(folder)) return std::nullopt;
  return folder;
}

}  // namespace

std::optional<std::wstring> AppDataFolder() {
  // Found (and made private) once; again only if it went missing meanwhile.
  static std::optional<std::wstring> folder;
  if (folder &&
      ::GetFileAttributesW(folder->c_str()) != INVALID_FILE_ATTRIBUTES) {
    return folder;
  }
  folder = FindFolder();
  return folder;
}

std::optional<std::wstring> AppDataFile(const wchar_t* name) {
  std::optional<std::wstring> folder = AppDataFolder();
  if (!folder) return std::nullopt;
  return *folder + L"\\" + name;
}
