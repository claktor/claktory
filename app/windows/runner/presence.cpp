#include "presence.h"

#include <dwmapi.h>
#include <wtsapi32.h>

#include "app_messages.h"

namespace {

// GUID_CONSOLE_DISPLAY_STATE, spelled out rather than linked: whether the
// display is on, off or dimmed.
constexpr GUID kDisplayState = {
    0x6fe69556, 0x704a, 0x47a0, {0x8f, 0x24, 0xc2, 0x8d, 0x93, 0x6f, 0xda, 0x47}};

// The window whose cloaking the hook reports; one window per process.
HWND g_cloak_window = nullptr;

// Told on the window's thread (WINEVENT_OUTOFCONTEXT), from its message
// loop: passed to the window as kPresenceChangedMessage.
void CALLBACK CloakChanged(HWINEVENTHOOK, DWORD, HWND window, LONG object,
                           LONG child, DWORD, DWORD) {
  if (window == nullptr || window != g_cloak_window || object != OBJID_WINDOW ||
      child != CHILDID_SELF) {
    return;
  }
  ::PostMessageW(window, kPresenceChangedMessage, 0, 0);
}

bool AnimationsOff() {
  BOOL on = TRUE;
  if (!::SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, &on, 0)) {
    return false;
  }
  return on == FALSE;
}

}  // namespace

Presence::Presence(HWND window) : window_(window) {
  HWND front = ::GetForegroundWindow();
  active_ = front != nullptr && ::GetAncestor(front, GA_ROOTOWNER) == window_;
  reduce_motion_ = AnimationsOff();
  session_notification_ =
      ::WTSRegisterSessionNotification(window_, NOTIFY_FOR_THIS_SESSION) != 0;
  display_notification_ = ::RegisterPowerSettingNotification(
      window_, &kDisplayState, DEVICE_NOTIFY_WINDOW_HANDLE);
  g_cloak_window = window_;
  cloak_hook_ = ::SetWinEventHook(EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED,
                                  nullptr, CloakChanged,
                                  ::GetCurrentProcessId(), 0,
                                  WINEVENT_OUTOFCONTEXT);
}

Presence::~Presence() {
  if (cloak_hook_ != nullptr) ::UnhookWinEvent(cloak_hook_);
  if (g_cloak_window == window_) g_cloak_window = nullptr;
  if (session_notification_) ::WTSUnRegisterSessionNotification(window_);
  if (display_notification_ != nullptr) {
    ::UnregisterPowerSettingNotification(display_notification_);
  }
}

bool Presence::Cloaked() const {
  DWORD cloaked = 0;
  return SUCCEEDED(::DwmGetWindowAttribute(window_, DWMWA_CLOAKED, &cloaked,
                                           sizeof(cloaked))) &&
         cloaked != 0;
}

bool Presence::Shown() const {
  if (locked_ || display_off_) return false;
  return ::IsWindowVisible(window_) && !::IsIconic(window_) && !Cloaked();
}

bool Presence::Using() const { return active_ && Shown(); }

bool Presence::OnWindowMessage(UINT message, WPARAM wparam, LPARAM lparam) {
  switch (message) {
    case WM_ACTIVATEAPP:
      // Told as it happens, where the foreground window may not have
      // changed yet.
      active_ = wparam != FALSE;
      return true;
    case WM_SIZE:
    case WM_SHOWWINDOW:
    case kPresenceChangedMessage:
      return true;
    case WM_SETTINGCHANGE:
      if (wparam == SPI_SETCLIENTAREAANIMATION) {
        reduce_motion_ = AnimationsOff();
        return true;
      }
      return false;
    case WM_WTSSESSION_CHANGE:
      if (wparam == WTS_SESSION_LOCK) {
        locked_ = true;
      } else if (wparam == WTS_SESSION_UNLOCK) {
        locked_ = false;
      }
      return true;
    case WM_POWERBROADCAST:
      if (wparam == PBT_POWERSETTINGCHANGE && lparam != 0) {
        auto setting = reinterpret_cast<const POWERBROADCAST_SETTING*>(lparam);
        if (setting->PowerSetting == kDisplayState &&
            setting->DataLength >= sizeof(DWORD)) {
          // 0 off, 1 on, 2 dimmed.
          DWORD state = *reinterpret_cast<const DWORD*>(setting->Data);
          display_off_ = state == 0;
          return true;
        }
      } else if (wparam == PBT_APMSUSPEND) {
        display_off_ = true;
        return true;
      } else if (wparam == PBT_APMRESUMEAUTOMATIC) {
        display_off_ = false;
        return true;
      }
      return false;
    default:
      return false;
  }
}
