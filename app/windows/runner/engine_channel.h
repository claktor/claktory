#ifndef RUNNER_ENGINE_CHANNEL_H_
#define RUNNER_ENGINE_CHANNEL_H_

#include <flutter/binary_messenger.h>
#include <flutter/encodable_value.h>
#include <flutter/method_channel.h>
#include <windows.h>

#include <memory>

// factory/engine on Windows, the Mac's Setup.swift Engine's counterpart:
// this PC's Claktory stopped and started from the app.
// - The window's system menu (its icon, or Alt+Space) has Stop Claktory or
//   Start Claktory as the app says with "menu" {title, enabled}, or none
//   for nil; choosing it sends "menuChosen", answered with why it was
//   refused, if it was, which a message box shows.
// - Closing the window first asks the app "quitting" and closes once it's
//   answered, or after kQuitWithinMs: the app stops Claktory first when the
//   person chose that.
class EngineChannel {
 public:
  EngineChannel(flutter::BinaryMessenger* messenger, HWND window);
  ~EngineChannel();

  // The window's messages this takes, before anything else does: true when
  // it took [message] and the window does nothing more with it.
  bool OnWindowMessage(UINT message, WPARAM wparam, LPARAM lparam);

 private:
  void SetItem(const flutter::EncodableValue* settings);
  void Chosen();
  void Close();

  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel_;
  HWND window_;
  bool item_shown_ = false;
  bool item_enabled_ = true;
  bool quitting_ = false;
  bool closing_ = false;
};

#endif  // RUNNER_ENGINE_CHANNEL_H_
