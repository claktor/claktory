#include "window_channel.h"

#include <flutter/standard_method_codec.h>

#include <string>
#include <variant>
#include <vector>

#include "drop_target.h"
#include "file_dialogs.h"

using flutter::EncodableList;
using flutter::EncodableValue;

WindowChannel::WindowChannel(flutter::BinaryMessenger* messenger, HWND window,
                             HWND view)
    : channel_(std::make_unique<flutter::MethodChannel<EncodableValue>>(
          messenger, "factory/window",
          &flutter::StandardMethodCodec::GetInstance())),
      window_(window),
      view_(view) {
  channel_->SetMethodCallHandler(
      [this](const auto& call, auto result) { Handle(call, std::move(result)); });

  auto* channel = channel_.get();
  drop_target_ = new DropTarget(
      view_,
      [channel](int count) {
        channel->InvokeMethod("filesOver",
                              std::make_unique<EncodableValue>(count));
      },
      [channel](std::vector<std::string> paths) {
        EncodableList list;
        for (auto& path : paths) list.emplace_back(std::move(path));
        channel->InvokeMethod("filesDropped",
                              std::make_unique<EncodableValue>(std::move(list)));
      });
  drop_target_->Register();
}

WindowChannel::~WindowChannel() {
  if (drop_target_ != nullptr) {
    drop_target_->Revoke();
    drop_target_->Release();
    drop_target_ = nullptr;
  }
}

void WindowChannel::Handle(
    const flutter::MethodCall<EncodableValue>& call,
    std::unique_ptr<flutter::MethodResult<EncodableValue>> result) {
  const std::string& method = call.method_name();
  const EncodableValue* arguments = call.arguments();

  if (method == "drag") {
    result->Success();
    Drag();
    return;
  }
  if (method == "zoom") {
    result->Success();
    ToggleMaximized();
    return;
  }
  if (method == "band") {
    if (arguments != nullptr) {
      if (const auto* height = std::get_if<double>(arguments)) {
        if (*height > 0) band_height_ = *height;
      }
    }
    result->Success();
    return;
  }
  if (method == "devMark") {
    result->Success();
    return;
  }
  if (method == "pickFolder") {
    std::string from;
    if (arguments != nullptr) {
      if (const auto* text = std::get_if<std::string>(arguments)) from = *text;
    }
    std::optional<std::string> folder = PickFolder(window_, from);
    if (folder) {
      result->Success(EncodableValue(*folder));
    } else {
      result->Success();
    }
    return;
  }
  if (method == "pickFiles") {
    EncodableList paths;
    for (auto& path : PickFiles(window_)) paths.emplace_back(std::move(path));
    result->Success(EncodableValue(std::move(paths)));
    return;
  }
  if (method == "takeFiles") {
    bool takes = false;
    if (arguments != nullptr) {
      if (const auto* value = std::get_if<bool>(arguments)) takes = *value;
    }
    if (drop_target_ != nullptr) drop_target_->set_takes_files(takes);
    result->Success();
    return;
  }
  result->NotImplemented();
}

void WindowChannel::Drag() {
  // The app asks once the pointer has moved with the button down; a late
  // request after the button came up would put the window in keyboard move
  // mode, so it's dropped.
  int button = ::GetSystemMetrics(SM_SWAPBUTTON) ? VK_RBUTTON : VK_LBUTTON;
  if ((::GetAsyncKeyState(button) & 0x8000) == 0) return;
  POINT pressed{};
  ::GetCursorPos(&pressed);
  ::ReleaseCapture();
  // As if the title bar itself were pressed: Windows' own move, with Snap,
  // and a maximized window restored under the pointer once it moves. It
  // returns once the button is released.
  ::SendMessageW(window_, WM_NCLBUTTONDOWN, HTCAPTION,
                 MAKELPARAM(pressed.x, pressed.y));
  // The move swallowed the button's release, so Flutter is told of it;
  // otherwise it would take the next click for the rest of a drag.
  POINT at{};
  ::GetCursorPos(&at);
  ::ScreenToClient(view_, &at);
  ::PostMessageW(view_, WM_LBUTTONUP, 0, MAKELPARAM(at.x, at.y));
}

void WindowChannel::ToggleMaximized() {
  ::ShowWindow(window_, ::IsZoomed(window_) ? SW_RESTORE : SW_MAXIMIZE);
}
