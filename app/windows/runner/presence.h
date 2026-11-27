#ifndef RUNNER_PRESENCE_H_
#define RUNNER_PRESENCE_H_

#include <windows.h>

// What the window knows of the person in front of it, for the channels that
// tell the app (notifications_channel.h's "using", sight_channel.h's
// "sight"):
// - whether the app is the one in front (WM_ACTIVATEAPP);
// - whether the session is locked (WTSRegisterSessionNotification);
// - whether the display is off (GUID_CONSOLE_DISPLAY_STATE, or the computer
//   asleep);
// - whether Windows hides the window on another virtual desktop (cloaked,
//   told by EVENT_OBJECT_CLOAKED and EVENT_OBJECT_UNCLOAKED);
// - whether the person turned Settings > Accessibility > Visual effects >
//   Animation effects off (SPI_GETCLIENTAREAANIMATION), which Flutter's
//   Windows embedder doesn't pass on (it passes high contrast only).
//
// Windows has no notice of a window covered by others (the Mac's
// occlusion), so a covered window still counts as shown; minimized, on
// another desktop, locked away or on a dark display it doesn't.
class Presence {
 public:
  explicit Presence(HWND window);
  ~Presence();

  Presence(const Presence&) = delete;
  Presence& operator=(const Presence&) = delete;

  // Takes the window's messages that may change any of it, and the runner's
  // own kPresenceChangedMessage; true when one may have changed, for the
  // channels to look again. Takes nothing from Flutter.
  bool OnWindowMessage(UINT message, WPARAM wparam, LPARAM lparam);

  // Any of the window is on a screen the person can see now.
  bool Shown() const;

  // Shown, and the app is the one in front: the person is using it.
  bool Using() const;

  // The person asked Windows for less motion.
  bool ReduceMotion() const { return reduce_motion_; }

 private:
  bool Cloaked() const;

  HWND window_;
  bool active_ = false;
  bool locked_ = false;
  bool display_off_ = false;
  bool reduce_motion_ = false;
  HPOWERNOTIFY display_notification_ = nullptr;
  bool session_notification_ = false;
  HWINEVENTHOOK cloak_hook_ = nullptr;
};

#endif  // RUNNER_PRESENCE_H_
