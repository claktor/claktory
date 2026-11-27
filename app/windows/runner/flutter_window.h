#ifndef RUNNER_FLUTTER_WINDOW_H_
#define RUNNER_FLUTTER_WINDOW_H_

#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>

#include <memory>

#include "device_channel.h"
#include "network_channel.h"
#include "notifications_channel.h"
#include "presence.h"
#include "sight_channel.h"
#include "win32_window.h"
#include "window_channel.h"

// The app's window: a Flutter view, and the channels the app's Dart side
// talks to Windows through (README.md lists them).
class FlutterWindow : public Win32Window {
 public:
  // Creates a new FlutterWindow hosting a Flutter view running |project|.
  explicit FlutterWindow(const flutter::DartProject& project);
  virtual ~FlutterWindow();

 protected:
  // Win32Window:
  bool OnCreate() override;
  void OnDestroy() override;
  LRESULT MessageHandler(HWND window, UINT const message, WPARAM const wparam,
                         LPARAM const lparam) noexcept override;

 private:
  // The project to run.
  flutter::DartProject project_;

  // The Flutter instance hosted by this window.
  std::unique_ptr<flutter::FlutterViewController> flutter_controller_;

  // What the window knows of the person in front of it, for the channels
  // below; made before them and gone after.
  std::unique_ptr<Presence> presence_;

  // The channels, made once the engine runs and gone before it is.
  std::unique_ptr<DeviceChannel> device_channel_;
  std::unique_ptr<WindowChannel> window_channel_;
  std::unique_ptr<NetworkChannel> network_channel_;
  std::unique_ptr<NotificationsChannel> notifications_channel_;
  std::unique_ptr<SightChannel> sight_channel_;
};

#endif  // RUNNER_FLUTTER_WINDOW_H_
