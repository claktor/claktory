#include "flutter_window.h"

#include <optional>
#include <string>

#include "app_messages.h"
#include "flutter/generated_plugin_registrant.h"
#include "utils.h"

FlutterWindow::FlutterWindow(const flutter::DartProject& project)
    : project_(project) {}

FlutterWindow::~FlutterWindow() {}

bool FlutterWindow::OnCreate() {
  if (!Win32Window::OnCreate()) {
    return false;
  }

  RECT frame = GetClientArea();

  // The size here must match the window dimensions to avoid unnecessary surface
  // creation / destruction in the startup path.
  flutter_controller_ = std::make_unique<flutter::FlutterViewController>(
      frame.right - frame.left, frame.bottom - frame.top, project_);
  // Ensure that basic setup of the controller was successful.
  if (!flutter_controller_->engine() || !flutter_controller_->view()) {
    return false;
  }
  RegisterPlugins(flutter_controller_->engine());
  SetChildContent(flutter_controller_->view()->GetNativeWindow());

  flutter::BinaryMessenger* messenger = flutter_controller_->engine()->messenger();
  device_channel_ = std::make_unique<DeviceChannel>(messenger);
  window_channel_ = std::make_unique<WindowChannel>(
      messenger, GetHandle(), flutter_controller_->view()->GetNativeWindow());
  network_channel_ = std::make_unique<NetworkChannel>(messenger, GetHandle());
  presence_ = std::make_unique<Presence>(GetHandle());
  notifications_channel_ =
      std::make_unique<NotificationsChannel>(messenger, GetHandle(), *presence_);
  sight_channel_ = std::make_unique<SightChannel>(messenger, *presence_);
  setup_channel_ = std::make_unique<SetupChannel>(messenger, GetHandle());
  engine_channel_ = std::make_unique<EngineChannel>(messenger, GetHandle());

  flutter_controller_->engine()->SetNextFrameCallback([&]() {
    this->Show();
    // How long the window took to show, from the process's start: what a
    // person waits through, logged for QA's timings.
    FILETIME created{}, exited{}, kernel{}, user{};
    if (::GetProcessTimes(::GetCurrentProcess(), &created, &exited, &kernel,
                          &user)) {
      FILETIME now{};
      ::GetSystemTimeAsFileTime(&now);
      ULARGE_INTEGER from{{created.dwLowDateTime, created.dwHighDateTime}};
      ULARGE_INTEGER to{{now.dwLowDateTime, now.dwHighDateTime}};
      Log("first frame shown after " +
          std::to_string((to.QuadPart - from.QuadPart) / 10000) + " ms");
    }
  });

  // Flutter can complete the first frame before the "show window" callback is
  // registered. The following call ensures a frame is pending to ensure the
  // window is shown. It is a no-op if the first frame hasn't completed yet.
  flutter_controller_->ForceRedraw();

  return true;
}

void FlutterWindow::OnDestroy() {
  engine_channel_ = nullptr;
  setup_channel_ = nullptr;
  sight_channel_ = nullptr;
  notifications_channel_ = nullptr;
  presence_ = nullptr;
  network_channel_ = nullptr;
  window_channel_ = nullptr;
  device_channel_ = nullptr;
  if (flutter_controller_) {
    flutter_controller_ = nullptr;
  }

  Win32Window::OnDestroy();
}

LRESULT
FlutterWindow::MessageHandler(HWND hwnd, UINT const message,
                              WPARAM const wparam,
                              LPARAM const lparam) noexcept {
  // What may change whether the person is using or seeing the app, looked
  // at before Flutter can take the message; it takes none itself.
  if (presence_ && presence_->OnWindowMessage(message, wparam, lparam)) {
    if (notifications_channel_) notifications_channel_->Changed();
    if (sight_channel_) sight_channel_->Changed();
  }

  // Closing asks the app first, and the system menu has Stop or Start
  // Claktory.
  if (engine_channel_ &&
      engine_channel_->OnWindowMessage(message, wparam, lparam)) {
    return 0;
  }

  // Give Flutter, including plugins, an opportunity to handle window messages.
  if (flutter_controller_) {
    std::optional<LRESULT> result =
        flutter_controller_->HandleTopLevelWindowProc(hwnd, message, wparam,
                                                      lparam);
    if (result) {
      return *result;
    }
  }

  switch (message) {
    case WM_FONTCHANGE:
      flutter_controller_->engine()->ReloadSystemFonts();
      break;
    case kNetworkChangedMessage:
      if (network_channel_) network_channel_->Changed();
      return 0;
    case kToastActivatedMessage:
      if (notifications_channel_) notifications_channel_->ToastsActivated();
      return 0;
    case kPresenceChangedMessage:
      return 0;
    case kSetupDoneMessage:
      if (setup_channel_) setup_channel_->Done(wparam);
      return 0;
  }

  return Win32Window::MessageHandler(hwnd, message, wparam, lparam);
}
