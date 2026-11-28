#include "engine_channel.h"

#include <flutter/method_result_functions.h>
#include <flutter/standard_method_codec.h>

#include <string>
#include <utility>
#include <variant>

#include "flavor.h"
#include "utils.h"

using flutter::EncodableMap;
using flutter::EncodableValue;

namespace {

// The system menu item's command: below 0xF000, where Windows' own are, with
// the low four bits clear, which Windows keeps for itself.
constexpr UINT kEngineItem = 0x0E10;

// The timer that closes the window when the app doesn't answer "quitting".
constexpr UINT_PTR kQuitTimer = 0x0E10;

// How long closing waits for the app, as the Mac's quitWithin.
constexpr UINT kQuitWithinMs = 5000;

}  // namespace

EngineChannel::EngineChannel(flutter::BinaryMessenger* messenger, HWND window)
    : window_(window) {
  channel_ = std::make_unique<flutter::MethodChannel<EncodableValue>>(
      messenger, "factory/engine",
      &flutter::StandardMethodCodec::GetInstance());
  channel_->SetMethodCallHandler([this](const auto& call, auto result) {
    if (call.method_name() == "menu") {
      SetItem(call.arguments());
      result->Success();
      return;
    }
    result->NotImplemented();
  });
}

EngineChannel::~EngineChannel() {
  channel_->SetMethodCallHandler(nullptr);
  ::KillTimer(window_, kQuitTimer);
}

void EngineChannel::SetItem(const EncodableValue* settings) {
  HMENU menu = ::GetSystemMenu(window_, FALSE);
  if (menu == nullptr) return;
  const EncodableMap* map =
      settings ? std::get_if<EncodableMap>(settings) : nullptr;
  const std::string* title = nullptr;
  if (map) {
    auto found = map->find(EncodableValue("title"));
    if (found != map->end()) title = std::get_if<std::string>(&found->second);
  }
  if (title == nullptr) {
    if (item_shown_) {
      ::DeleteMenu(menu, kEngineItem, MF_BYCOMMAND);
      // Its separator, just above Close's.
      int count = ::GetMenuItemCount(menu);
      if (count >= 2) ::DeleteMenu(menu, count - 2, MF_BYPOSITION);
      item_shown_ = false;
    }
    return;
  }
  item_enabled_ = true;
  auto enabled = map->find(EncodableValue("enabled"));
  if (enabled != map->end()) {
    if (const bool* on = std::get_if<bool>(&enabled->second)) {
      item_enabled_ = *on;
    }
  }
  std::wstring text = Utf16FromUtf8(*title);
  UINT flags = MF_BYCOMMAND | MF_STRING | (item_enabled_ ? MF_ENABLED : MF_GRAYED);
  if (item_shown_) {
    ::ModifyMenuW(menu, kEngineItem, flags, kEngineItem, text.c_str());
    return;
  }
  // Above Close, with a separator of its own.
  int count = ::GetMenuItemCount(menu);
  int at = count > 0 ? count - 1 : 0;
  ::InsertMenuW(menu, at, MF_BYPOSITION | MF_STRING |
                              (item_enabled_ ? MF_ENABLED : MF_GRAYED),
                kEngineItem, text.c_str());
  ::InsertMenuW(menu, at + 1, MF_BYPOSITION | MF_SEPARATOR, 0, nullptr);
  item_shown_ = true;
}

void EngineChannel::Chosen() {
  HWND window = window_;
  channel_->InvokeMethod(
      "menuChosen", nullptr,
      std::make_unique<flutter::MethodResultFunctions<EncodableValue>>(
          [window](const EncodableValue* answer) {
            const std::string* words =
                answer ? std::get_if<std::string>(answer) : nullptr;
            if (words == nullptr || words->empty()) return;
            std::wstring text = Utf16FromUtf8(*words);
            ::MessageBoxW(window, text.c_str(), L"Couldn't stop Claktory",
                          MB_OK | MB_ICONINFORMATION);
          },
          nullptr, nullptr));
}

void EngineChannel::Close() {
  if (closing_) return;
  closing_ = true;
  ::KillTimer(window_, kQuitTimer);
  ::PostMessageW(window_, WM_CLOSE, 0, 0);
}

bool EngineChannel::OnWindowMessage(UINT message, WPARAM wparam, LPARAM) {
  switch (message) {
    case WM_SYSCOMMAND:
      if ((wparam & 0xFFF0) == kEngineItem) {
        if (item_enabled_) Chosen();
        return true;
      }
      return false;
    case WM_CLOSE:
      if (closing_) return false;
      if (!quitting_) {
        quitting_ = true;
        ::SetTimer(window_, kQuitTimer, kQuitWithinMs, nullptr);
        channel_->InvokeMethod(
            "quitting", nullptr,
            std::make_unique<flutter::MethodResultFunctions<EncodableValue>>(
                [this](const EncodableValue*) { Close(); },
                [this](const std::string&, const std::string&,
                       const EncodableValue*) { Close(); },
                [this]() { Close(); }));
      }
      return true;
    case WM_TIMER:
      if (wparam == kQuitTimer) {
        Close();
        return true;
      }
      return false;
  }
  return false;
}
