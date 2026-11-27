#ifndef RUNNER_TOAST_ACTIVATOR_H_
#define RUNNER_TOAST_ACTIVATOR_H_

#include <windows.h>

#include <deque>
#include <memory>
#include <mutex>
#include <string>

// The tags of toasts clicked, from the threads Windows tells on to the
// window's (kToastActivatedMessage, notifications_channel.h).
struct ActivationQueue {
  std::mutex mutex;
  std::deque<std::string> tags;
  HWND window = nullptr;
};

// The COM class Windows starts or calls when one of the app's toasts is
// clicked (INotificationActivationCallback, flavor.h's kToastActivator):
// while the app runs, and when it doesn't, by starting it with -Embedding.
// The app registers the class for this account at start
// (notifications_channel.cpp: HKCU\Software\Classes\CLSID\<it>\LocalServer32
// and the AppUserModelId's CustomActivator, as the Windows App SDK does for
// unpackaged apps); this object answers for it while the app runs.
//
// What a click carries is the toast's launch argument, its tag alone: the
// tag is looked up among the toasts this run showed, so any program of this
// account that calls the class can only bring the window forward or open
// what a toast already offered.
class ToastActivator {
 public:
  explicit ToastActivator(std::shared_ptr<ActivationQueue> queue);
  ~ToastActivator();

  ToastActivator(const ToastActivator&) = delete;
  ToastActivator& operator=(const ToastActivator&) = delete;

  // Windows calls this process for clicks; false when the class couldn't be
  // registered (the toasts' own Activated event is the way then, while the
  // app runs).
  bool registered() const { return cookie_ != 0; }

 private:
  DWORD cookie_ = 0;
};

// Whether [argument] is what COM adds to the command line when it starts the
// app for a click ("-Embedding" or "/Embedding"), which the app's Dart side
// never sees.
bool IsComStartArgument(const std::string& argument);

#endif  // RUNNER_TOAST_ACTIVATOR_H_
