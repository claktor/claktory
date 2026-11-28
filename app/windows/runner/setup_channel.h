#ifndef RUNNER_SETUP_CHANNEL_H_
#define RUNNER_SETUP_CHANNEL_H_

#include <flutter/binary_messenger.h>
#include <flutter/encodable_value.h>
#include <flutter/method_channel.h>
#include <windows.h>

#include <map>
#include <memory>
#include <mutex>
#include <string>

// factory/setup on Windows, the Mac's Setup.swift's counterpart: this PC's
// own Claktory, set up, started and checked by the backend itself
// (`factoryd install --check | --start | --repair`, and `uninstall`), which
// answers one JSON line the Dart side reads as on the Mac. The backend owns
// the sign-in task that runs it; the installer asked the person about it.
// - location: where the app runs, and whether there's a backend to run;
// - install, uninstall: runs factoryd.exe with the arguments, which must
//   start with that word, with no window, and answers its exit code and
//   output;
// - appOnly, setAppOnly: whether this PC has no Claktory of its own, only
//   the app, kept in the app's own folder (app_paths.h);
// - backgroundStatus: "unavailable", as Windows has no switch like the Mac's
//   Allow in the Background; openLoginItems does nothing.
// removeApp and moveToApplications aren't Windows': Installed apps removes
// Claktory there, and the installer puts it where it stays.
class SetupChannel {
 public:
  // [window] gets kSetupDoneMessage when a run of factoryd.exe ends.
  SetupChannel(flutter::BinaryMessenger* messenger, HWND window);
  ~SetupChannel();

  // A run ended: answers its call. From the window's kSetupDoneMessage.
  void Done(WPARAM run);

 private:
  struct Run {
    std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result;
    // Set by the run's thread before it posts kSetupDoneMessage.
    bool started = false;
    std::string failure;
    unsigned long code = 0;
    std::string out;
    std::string err;
  };

  void Handle(const flutter::MethodCall<flutter::EncodableValue>& call,
              std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
                  result);

  // Runs [exe] with [arguments] on a thread of its own.
  void Start(const std::wstring& exe, const std::vector<std::string>& arguments,
             std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
                 result);

  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel_;
  HWND window_;
  std::mutex runs_lock_;
  std::map<WPARAM, std::shared_ptr<Run>> runs_;
  WPARAM next_run_ = 1;
};

#endif  // RUNNER_SETUP_CHANNEL_H_
