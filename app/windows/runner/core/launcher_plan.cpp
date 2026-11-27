#include "core/launcher_plan.h"

namespace claktory {

bool IsAppVersion(const std::string& text) {
  if (text.empty() || text.size() > kMostVersionLength) return false;
  bool digit_before = false;
  for (char c : text) {
    if (c >= '0' && c <= '9') {
      digit_before = true;
    } else if (c == '.' && digit_before) {
      digit_before = false;
    } else {
      return false;
    }
  }
  // Not ending in a dot.
  return digit_before;
}

std::string PointedVersion(const std::string& bytes) {
  auto space = [](char c) {
    return c == ' ' || c == '\t' || c == '\r' || c == '\n';
  };
  std::size_t start = 0;
  std::size_t end = bytes.size();
  while (start < end && space(bytes[start])) ++start;
  while (end > start && space(bytes[end - 1])) --end;
  std::string version = bytes.substr(start, end - start);
  return IsAppVersion(version) ? version : std::string();
}

bool FailedToStart(bool exited, unsigned long exit_code) {
  return exited && exit_code != 0;
}

std::vector<std::string> StaleVersions(const std::vector<std::string>& folders,
                                       const std::string& current,
                                       const std::string& previous) {
  std::vector<std::string> stale;
  for (const auto& name : folders) {
    if (!IsAppVersion(name)) continue;
    if (name == current || name == previous) continue;
    stale.push_back(name);
  }
  return stale;
}

unsigned long TakeAfter(std::wstring* arguments) {
  const std::wstring flag = L"--after ";
  if (arguments->compare(0, flag.size(), flag) != 0) return 0;
  std::size_t at = flag.size();
  while (at < arguments->size() && (*arguments)[at] == L' ') ++at;
  unsigned long long pid = 0;
  std::size_t digits = 0;
  while (at < arguments->size() && (*arguments)[at] >= L'0' &&
         (*arguments)[at] <= L'9') {
    if (digits == 10) return 0;
    pid = pid * 10 + static_cast<unsigned long long>((*arguments)[at] - L'0');
    ++at;
    ++digits;
  }
  if (digits == 0 || pid == 0 || pid > 0xFFFFFFFFull) return 0;
  if (at < arguments->size() && (*arguments)[at] != L' ' &&
      (*arguments)[at] != L'\t') {
    return 0;
  }
  while (at < arguments->size() &&
         ((*arguments)[at] == L' ' || (*arguments)[at] == L'\t')) {
    ++at;
  }
  arguments->erase(0, at);
  return static_cast<unsigned long>(pid);
}

}  // namespace claktory
