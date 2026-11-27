// What the runner keeps on disk, checked on Windows itself: the app's
// private folder (owner, protected DACL, junctions and loose DACLs refused or
// tightened), user DPAPI round trips, the connection store's rules over the
// real DpapiFile, and the plain JSON files. A console program with no
// window, so it runs over ssh as the account under test.
//
// It works in the debug flavor's folder (%LOCALAPPDATA%\Claktory\app-debug):
// it removes that folder first and last, so run it where no debug build's
// connections matter. Each part runs in a process of its own (the app finds
// its folder once per process), the program starting itself for each.
//
// DPAPI needs the account's own sign-in: over ssh with a key, Windows has no
// password to open the account's master key with, and CryptProtectData
// answers "Access is denied". The store's part then says SKIPPED (exit 2)
// rather than failing; run it in the account's desktop session for those.
//
// Built by app/windows/tool/cross_build.sh on a Mac (beside the app).
#include <windows.h>
#include <aclapi.h>
#include <sddl.h>
#include <shlobj.h>

#include <cstdio>
#include <memory>
#include <iterator>
#include <string>

#include "app_paths.h"
#include "core/connection_store.h"
#include "core/json.h"
#include "file_store.h"
#include "flavor.h"
#include "utils.h"

using claktory::ConnectionStore;
using claktory::Json;

namespace {

int checks = 0;
int failures = 0;

void Check(bool condition, const char* what) {
  ++checks;
  if (!condition) {
    ++failures;
    std::printf("FAIL: %s\n", what);
  } else {
    std::printf("ok: %s\n", what);
  }
}

std::wstring ClaktoryFolder() {
  PWSTR local = nullptr;
  std::wstring folder;
  if (SUCCEEDED(::SHGetKnownFolderPath(FOLDERID_LocalAppData, 0, nullptr,
                                       &local))) {
    folder = std::wstring(local) + L"\\Claktory";
  }
  ::CoTaskMemFree(local);
  return folder;
}

std::wstring TestFolder() {
  return ClaktoryFolder() + L"\\" + flavor::kDataFolder;
}

// Removes [folder] and what's in it (a junction itself, never its target).
void RemoveTree(const std::wstring& folder) {
  DWORD attributes = ::GetFileAttributesW(folder.c_str());
  if (attributes == INVALID_FILE_ATTRIBUTES) return;
  if (attributes & FILE_ATTRIBUTE_REPARSE_POINT) {
    ::RemoveDirectoryW(folder.c_str());
    return;
  }
  WIN32_FIND_DATAW found;
  HANDLE find = ::FindFirstFileW((folder + L"\\*").c_str(), &found);
  if (find != INVALID_HANDLE_VALUE) {
    do {
      std::wstring name = found.cFileName;
      if (name == L"." || name == L"..") continue;
      std::wstring path = folder + L"\\" + name;
      if (found.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) {
        RemoveTree(path);
      } else {
        ::SetFileAttributesW(path.c_str(), FILE_ATTRIBUTE_NORMAL);
        ::DeleteFileW(path.c_str());
      }
    } while (::FindNextFileW(find, &found));
    ::FindClose(find);
  }
  ::RemoveDirectoryW(folder.c_str());
}

// [path]'s owner and DACL as SDDL ("O:...D:..."), empty when unreadable.
std::wstring Sddl(const std::wstring& path) {
  PSECURITY_DESCRIPTOR descriptor = nullptr;
  if (::GetNamedSecurityInfoW(path.c_str(), SE_FILE_OBJECT,
                              OWNER_SECURITY_INFORMATION |
                                  DACL_SECURITY_INFORMATION,
                              nullptr, nullptr, nullptr, nullptr,
                              &descriptor) != ERROR_SUCCESS) {
    return std::wstring();
  }
  LPWSTR text = nullptr;
  std::wstring sddl;
  if (::ConvertSecurityDescriptorToStringSecurityDescriptorW(
          descriptor, SDDL_REVISION_1,
          OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION, &text,
          nullptr)) {
    sddl = text;
    ::LocalFree(text);
  }
  // SetNamedSecurityInfo marks a DACL it sets auto-inherited (AI), which
  // changes no one's access: compared without it.
  size_t mark = sddl.find(L"D:PAI");
  if (mark != std::wstring::npos) sddl.erase(mark + 3, 2);
  ::LocalFree(descriptor);
  return sddl;
}

std::wstring UserSid() {
  HANDLE token = nullptr;
  std::wstring sid;
  if (!::OpenProcessToken(::GetCurrentProcess(), TOKEN_QUERY, &token)) {
    return sid;
  }
  BYTE buffer[256];
  DWORD size = 0;
  if (::GetTokenInformation(token, TokenUser, buffer, sizeof(buffer), &size)) {
    LPWSTR text = nullptr;
    if (::ConvertSidToStringSidW(reinterpret_cast<TOKEN_USER*>(buffer)->User.Sid,
                                 &text)) {
      sid = text;
      ::LocalFree(text);
    }
  }
  ::CloseHandle(token);
  return sid;
}

// Runs [command] with no window; its exit code, or -1.
int Run(std::wstring command) {
  STARTUPINFOW startup{};
  startup.cb = sizeof(startup);
  PROCESS_INFORMATION process{};
  if (!::CreateProcessW(nullptr, command.data(), nullptr, nullptr, TRUE, 0,
                        nullptr, nullptr, &startup, &process)) {
    return -1;
  }
  ::WaitForSingleObject(process.hProcess, 60000);
  DWORD code = static_cast<DWORD>(-1);
  ::GetExitCodeProcess(process.hProcess, &code);
  ::CloseHandle(process.hProcess);
  ::CloseHandle(process.hThread);
  return static_cast<int>(code);
}

void Print(const char* label, const std::wstring& text) {
  std::printf("  %s %s\n", label, Utf8FromUtf16(text.c_str()).c_str());
}

std::wstring WantedSddl() {
  std::wstring user = UserSid();
  return L"O:" + user + L"D:P(A;OICI;FA;;;" + user + L")(A;OICI;FA;;;SY)";
}

void FolderNew() {
  std::wstring folder = TestFolder();
  RemoveTree(folder);
  auto made = AppDataFolder();
  Check(made.has_value() && *made == folder, "a new folder is made");
  std::wstring sddl = Sddl(folder);
  Print("new folder:", sddl);
  Check(sddl == WantedSddl(),
        "a new folder: owned by the user, only user and SYSTEM");
}

// One already there with what LocalAppData passes down (Administrators
// too): taken, and made private again.
void FolderExisting() {
  std::wstring folder = TestFolder();
  RemoveTree(folder);
  ::CreateDirectoryW(folder.c_str(), nullptr);
  Print("inherited:", Sddl(folder));
  auto taken = AppDataFolder();
  std::wstring sddl = Sddl(folder);
  Print("taken:", sddl);
  Check(taken.has_value(), "an existing plain folder is used");
  Check(sddl == WantedSddl(), "an existing folder's DACL is made private");
}

// A junction in its place (to another folder of the user's, say) is refused.
void FolderJunction() {
  std::wstring folder = TestFolder();
  RemoveTree(folder);
  std::wstring elsewhere = ClaktoryFolder() + L"\\smoke-elsewhere";
  RemoveTree(elsewhere);
  ::CreateDirectoryW(elsewhere.c_str(), nullptr);
  std::wstring command = L"cmd.exe /c mklink /J \"" + folder + L"\" \"" +
                         elsewhere + L"\" >nul";
  bool linked = Run(command) == 0;
  Check(linked, "(setup) a junction made in the folder's place");
  if (linked) {
    Check(!AppDataFolder().has_value(), "a junction in its place is refused");
    Check(!AppDataFile(L"connections.dpapi").has_value(),
          "so no file is kept through it");
    WIN32_FIND_DATAW found;
    HANDLE inside = ::FindFirstFileW((elsewhere + L"\\*.dpapi").c_str(), &found);
    Check(inside == INVALID_HANDLE_VALUE, "and nothing lands at its target");
    if (inside != INVALID_HANDLE_VALUE) ::FindClose(inside);
  }
  RemoveTree(folder);
  RemoveTree(elsewhere);
}

// Exit code for a part that couldn't run here.
constexpr int kSkipped = 2;
bool skipped = false;

void StoreChecks() {
  RemoveTree(TestFolder());
  std::string logged;
  auto log = [&logged](const std::string& line) { logged += line + "\n"; };

  {
    ConnectionStore store(std::make_unique<DpapiFile>(L"connections.dpapi"),
                          log);
    Json connections = Json::MakeMap(
        {{"ws://127.0.0.1:7420", Json::String("secret-token-smoke-check")}});
    auto refused = store.Write(connections);
    Check(refused.has_value() && refused->code == "credentials_read_failed",
          "no write before the first read");
    auto first = store.Read();
    Check(std::holds_alternative<Json>(first) &&
              std::get<Json>(first).is_map() &&
              std::get<Json>(first).map().empty(),
          "nothing kept reads as no connections");
    // Whether DPAPI works in this sign-in at all, asked of DPAPI itself.
    DATA_BLOB plain{5, reinterpret_cast<BYTE*>(const_cast<char*>("probe"))};
    DATA_BLOB sealed{};
    if (!::CryptProtectData(&plain, nullptr, nullptr, nullptr, nullptr,
                            CRYPTPROTECT_UI_FORBIDDEN, &sealed)) {
      std::printf(
          "SKIPPED: DPAPI isn't open in this sign-in (%s); run the store's "
          "part in the account's desktop session\n",
          Win32ErrorMessage(::GetLastError()).c_str());
      auto failed = store.Write(connections);
      Check(failed.has_value() && failed->code == "credentials_write_failed",
            "a write DPAPI refuses fails as credentials_write_failed");
      skipped = true;
      return;
    }
    ::LocalFree(sealed.pbData);
    Check(!store.Write(connections).has_value(), "then it's kept");
    auto again = store.Read();
    Check(std::holds_alternative<Json>(again) &&
              std::get<Json>(again) == connections,
          "and reads back the same (DPAPI round trip)");
  }

  auto path = AppDataFile(L"connections.dpapi");
  Check(path.has_value(), "the store's file has a path");
  if (!path) return;
  FileRead raw = ReadWholeFile(*path);
  Check(raw.status == FileRead::Status::kFound, "the file is there");
  if (raw.data.size() < 32) return;
  Check(raw.data.rfind("CLAKTORY-DPAPI-1\n", 0) == 0, "with its header");
  Check(raw.data.find("secret-token-smoke-check") == std::string::npos &&
            raw.data.find("127.0.0.1") == std::string::npos,
        "and no plain text in it");
  std::wstring sddl = Sddl(*path);
  Print("file:", sddl);
  Check(sddl.find(L"BA") == std::string::npos &&
            sddl.find(L"BU") == std::string::npos &&
            sddl.find(L"AU") == std::string::npos,
        "the file inherits the private DACL (no Administrators or Users)");
  WIN32_FIND_DATAW found;
  HANDLE copies = ::FindFirstFileW((*path + L".*.new").c_str(), &found);
  Check(copies == INVALID_HANDLE_VALUE, "no copy left beside it");
  if (copies != INVALID_HANDLE_VALUE) ::FindClose(copies);

  // A damaged blob: unreadable, and never written over.
  std::string damaged = raw.data;
  damaged[damaged.size() / 2] ^= 0x5A;
  WriteWholeFile(*path, damaged);
  {
    ConnectionStore store(std::make_unique<DpapiFile>(L"connections.dpapi"),
                          log);
    auto read = store.Read();
    Check(std::holds_alternative<claktory::StoreFailure>(read) &&
              std::get<claktory::StoreFailure>(read).code ==
                  "credentials_unreadable",
          "a damaged blob reads as unreadable");
    auto refused = store.Write(Json::MakeMap());
    Check(refused.has_value() && refused->code == "credentials_read_failed",
          "and isn't written over");
    Check(ReadWholeFile(*path).data == damaged, "it's left as it was");
    Check(!store.Reset().has_value(), "starting over removes it");
    Check(ReadWholeFile(*path).status == FileRead::Status::kMissing,
          "it's gone");
    Check(!store.Write(Json::MakeMap({{"a", Json::String("b")}})).has_value(),
          "and writing works again");
  }

  // Not a blob of ours at all.
  WriteWholeFile(*path, "not dpapi");
  {
    ConnectionStore store(std::make_unique<DpapiFile>(L"connections.dpapi"),
                          log);
    auto read = store.Read();
    Check(std::holds_alternative<claktory::StoreFailure>(read) &&
              std::get<claktory::StoreFailure>(read).code ==
                  "credentials_unreadable",
          "a file without the header reads as unreadable");
  }
  std::printf("  store log:\n%s", logged.c_str());
}

void JsonFileChecks() {
  JsonFile drafts(L"drafts.json");
  Json value = Json::MakeMap({{"agent", Json::String("half a message")}});
  Check(!drafts.Write(value).has_value(), "drafts are kept");
  auto back = drafts.Read();
  Check(back.has_value() && *back == value, "and read back");
}

struct Part {
  const char* name;
  void (*run)();
};

constexpr Part kParts[] = {
    {"folder-new", FolderNew},
    {"folder-existing", FolderExisting},
    {"folder-junction", FolderJunction},
    {"store", StoreChecks},
    {"json", JsonFileChecks},
};

}  // namespace

int wmain(int argc, wchar_t** argv) {
  ::CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
  if (argc > 1) {
    std::string wanted = Utf8FromUtf16(argv[1]);
    for (const Part& part : kParts) {
      if (wanted != part.name) continue;
      std::printf("== %s\n", part.name);
      part.run();
      std::printf("%d checks, %d failed\n", checks, failures);
      std::fflush(stdout);
      return failures > 0 ? 1 : skipped ? kSkipped : 0;
    }
    std::printf("no part %s\n", wanted.c_str());
    return 1;
  }
  std::printf("flavor folder: %s\n",
              Utf8FromUtf16(TestFolder().c_str()).c_str());
  std::fflush(stdout);
  wchar_t self[MAX_PATH * 4];
  DWORD length = ::GetModuleFileNameW(nullptr, self,
                                      static_cast<DWORD>(std::size(self)));
  int failed = 0;
  int skipped_parts = 0;
  for (const Part& part : kParts) {
    std::wstring command = L"\"" + std::wstring(self, length) + L"\" " +
                           Utf16FromUtf8(part.name);
    int code = Run(command);
    if (code == kSkipped) {
      ++skipped_parts;
    } else if (code != 0) {
      ++failed;
      std::printf("PART FAILED: %s (%d)\n", part.name, code);
    }
  }
  RemoveTree(TestFolder());
  std::printf("%d parts: %d failed, %d skipped\n",
              static_cast<int>(std::size(kParts)), failed, skipped_parts);
  ::CoUninitialize();
  return failed > 0 ? 1 : 0;
}
