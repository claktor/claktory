#ifndef RUNNER_CORE_LAUNCHER_PLAN_H_
#define RUNNER_CORE_LAUNCHER_PLAN_H_

// The launcher's rules (windows-updates.md section 5), apart from Windows so
// they're tested anywhere (core_test.cpp).
//
// The app lives in %LOCALAPPDATA%\Programs\Claktory\app\<version>\, one
// folder per version, beside a small launcher (launcher/main.cpp) that the
// Start menu's shortcut opens. The launcher starts only the version
// app\current names, a file the updater writes (atomically) once that
// version's files passed the release's hash and its publisher's signature,
// and the installer writes for the version it lays down. It never picks a
// folder by its own reading of the names. When that version fails to start,
// it goes back to app\previous, the one the updater replaced, and points
// current at it. It deletes the versions neither names; the updater never
// deletes one. While the updater lays a version down it holds
// app\installing open, and the launcher deletes nothing then, so a version
// whose pointer isn't written yet is never taken for a stale one.

#include <string>
#include <vector>

namespace claktory {

// The longest version a pointer may name.
constexpr std::size_t kMostVersionLength = 32;

// Whether [text] is a version as the app's folders are named: digits in
// groups split by single dots ("0.1.27"), at most kMostVersionLength long.
// Nothing else can name a folder: no separator, "..", drive or space.
bool IsAppVersion(const std::string& text);

// The version a pointer file's [bytes] name: the file's text with spaces
// and line breaks around it trimmed; "" when that isn't a version.
std::string PointedVersion(const std::string& bytes);

// Whether a version's app failed to start: it exited within the launcher's
// wait ([exited]) with anything but success. A second start that hands
// over to the app already running exits 0, which is fine.
bool FailedToStart(bool exited, unsigned long exit_code);

// Of the folders in app\ ([folders], their names), the ones the launcher
// deletes: versions that are neither [current] nor [previous]. A name that
// isn't a version is never one of them, whatever it is.
std::vector<std::string> StaleVersions(const std::vector<std::string>& folders,
                                       const std::string& current,
                                       const std::string& previous);

// The process to wait for before starting: the number after "--after" at
// the start of [arguments] (an app that just updated starts the launcher so
// and quits), taken out of them with the spaces after it. 0, and
// [arguments] unchanged, when they don't start so.
unsigned long TakeAfter(std::wstring* arguments);

// Where the launcher would be for the app at [exe]: <root>\Claktory.exe for
// an exe in <root>\app\<version>\, "" for an exe anywhere else (a build, an
// unzipped bundle). Whether it's there is the caller's to check.
std::wstring LauncherFor(const std::wstring& exe);

}  // namespace claktory

#endif  // RUNNER_CORE_LAUNCHER_PLAN_H_
