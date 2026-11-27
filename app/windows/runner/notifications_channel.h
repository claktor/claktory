#ifndef RUNNER_NOTIFICATIONS_CHANNEL_H_
#define RUNNER_NOTIFICATIONS_CHANNEL_H_

#include <flutter/binary_messenger.h>
#include <flutter/encodable_value.h>
#include <flutter/method_channel.h>
#include <windows.h>

#include <deque>
#include <map>
#include <memory>
#include <string>

#include "presence.h"

// factory/notifications on Windows (lib/notifications.dart's
// MacNotifications contract; the Mac's Notifications.swift): what a
// Claktory's backend says the person should hear of, as Windows toasts, and
// whether the person is using the app, so the backend notifies no one while
// they are.
// - "listen": answers whether the person is using the app now, and starts
//   the calls back, any click that came before first;
// - "post" {id, title, body, thread, urgent, target}: shows it; one with an
//   id already shown replaces it (the toast's Tag; Group is the thread);
//   urgent ones get the high priority Windows gives a call or an alarm;
// - "permission": "allowed" or "denied", as Windows' Settings > System >
//   Notifications says for Claktory (Windows never asks, so "ask" answers
//   the same);
// - "openSettings": that page;
// - "using" (to the app): presence.h's Using(), the app in front and shown;
//   sent as it changes;
// - "clicked" (to the app) target: a toast was clicked; the window comes
//   forward first (Windows starts the app if it isn't running,
//   toast_activator.h). A toast's target stays in this process: nothing
//   but its id goes into the toast (windows.md: no token in a toast's
//   arguments), so a click on one left from an earlier run brings the
//   window forward and opens nothing.
//
// Windows shows toasts only for an app it knows by its AppUserModelID
// (flavor.h): the app registers it for this account at start
// (HKCU\Software\Classes\AppUserModelId\<id>: its name, picture and click
// activator, whose CLSID key names this exe), as the Windows App SDK does
// for unpackaged apps; the uninstaller removes both keys (README.md).
class NotificationsChannel {
 public:
  NotificationsChannel(flutter::BinaryMessenger* messenger, HWND window,
                       const Presence& presence);
  ~NotificationsChannel();

  // Looks again at "using", after presence.h said it may have changed.
  void Changed();

  // The runner's own kToastActivatedMessage: toasts clicked.
  void ToastsActivated();

 private:
  struct Toasts;

  void Handle(const flutter::MethodCall<flutter::EncodableValue>& call,
              std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
                  result);
  void Post(const flutter::EncodableMap& notification);
  std::string Permission();
  void Clicked(const std::string& tag);

  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel_;
  HWND window_;
  const Presence& presence_;
  std::unique_ptr<Toasts> toasts_;
  bool listening_ = false;
  bool said_ = false;
  // Targets of the toasts shown, by tag, while the app runs.
  std::map<std::string, flutter::EncodableValue> targets_;
  std::deque<std::string> target_order_;
  // Clicks before "listen".
  std::deque<flutter::EncodableValue> clicks_;
};

#endif  // RUNNER_NOTIFICATIONS_CHANNEL_H_
