#ifndef RUNNER_CORE_COMMAND_LINE_H_
#define RUNNER_CORE_COMMAND_LINE_H_

#include <string>
#include <vector>

namespace claktory {

// The command line that starts [program] with [arguments], each one quoted
// as Windows' own parsing (CommandLineToArgvW, and the C runtime's) reads it
// back unchanged: spaces, tabs, quotes and the backslashes before them
// included.
std::wstring CommandLine(const std::wstring& program,
                         const std::vector<std::wstring>& arguments);

}  // namespace claktory

#endif  // RUNNER_CORE_COMMAND_LINE_H_
