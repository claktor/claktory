#ifndef RUNNER_WINDOW_CHANNEL_H_
#define RUNNER_WINDOW_CHANNEL_H_

#include <flutter/binary_messenger.h>
#include <flutter/encodable_value.h>
#include <flutter/method_channel.h>
#include <windows.h>

#include <memory>

class DropTarget;

// factory/window on Windows, the Mac's MainFlutterWindow.swift's
// counterpart:
// - drag: the app's header bars and empty bands move the window (lib/
//   controls.dart's WindowDrag asks once the pointer moves); Windows' own
//   move, so dragging to a screen's edge snaps it;
// - zoom: what double-clicking a title bar does on Windows: maximize, or
//   restore a maximized window;
// - band: the height of the app's top band; kept for the title bar
//   (README.md, "Title bar");
// - devMark: nothing to do (the Dart side draws Claktory Dev's mark here:
//   Windows has no native views for it to hide under);
// - pickFolder (a starting folder) and pickFiles: the system's Open dialog
//   (file_dialogs.h);
// - takeFiles: whether files dropped on the window go to the message box
//   showing; the window then tells "filesOver" (how many) and "filesDropped"
//   (their paths) back on the channel (drop_target.h).
class WindowChannel {
 public:
  // [window] is the top-level window, [view] the Flutter view inside it.
  WindowChannel(flutter::BinaryMessenger* messenger, HWND window, HWND view);
  ~WindowChannel();

  // The app's top band's height in logical pixels, as the app last said.
  double band_height() const { return band_height_; }

 private:
  void Handle(const flutter::MethodCall<flutter::EncodableValue>& call,
              std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
                  result);

  // Moves the window with the pointer until the button is released, if it's
  // still down.
  void Drag();
  void ToggleMaximized();

  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel_;
  HWND window_;
  HWND view_;
  DropTarget* drop_target_ = nullptr;
  double band_height_ = 44;
};

#endif  // RUNNER_WINDOW_CHANNEL_H_
