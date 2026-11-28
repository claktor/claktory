#include "core/command_line.h"

namespace claktory {

namespace {

void Append(std::wstring* line, const std::wstring& argument) {
  if (!argument.empty() &&
      argument.find_first_of(L" \t\n\v\"") == std::wstring::npos) {
    *line += argument;
    return;
  }
  *line += L'"';
  std::size_t backslashes = 0;
  for (wchar_t c : argument) {
    if (c == L'\\') {
      ++backslashes;
      continue;
    }
    if (c == L'"') {
      // Each backslash before a quote doubled, and the quote escaped.
      line->append(backslashes * 2 + 1, L'\\');
    } else {
      line->append(backslashes, L'\\');
    }
    backslashes = 0;
    *line += c;
  }
  // Doubled before the closing quote, so they don't escape it.
  line->append(backslashes * 2, L'\\');
  *line += L'"';
}

}  // namespace

std::wstring CommandLine(const std::wstring& program,
                         const std::vector<std::wstring>& arguments) {
  // The program's own name is read up to the next quote, with no escapes.
  std::wstring line = L"\"" + program + L"\"";
  for (const auto& argument : arguments) {
    line += L' ';
    Append(&line, argument);
  }
  return line;
}

}  // namespace claktory
