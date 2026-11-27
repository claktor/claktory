#ifndef RUNNER_APP_MESSAGES_H_
#define RUNNER_APP_MESSAGES_H_

#include <windows.h>

// The runner's own window messages: what the system tells on its own threads
// is posted to the window, so the channels answer the Dart side on the
// platform thread, as Flutter requires.

// The network changed (network_channel.h).
constexpr UINT kNetworkChangedMessage = WM_APP + 1;

// A toast was clicked (notifications_channel.h).
constexpr UINT kToastActivatedMessage = WM_APP + 2;

// The window was cloaked or uncloaked (presence.h).
constexpr UINT kPresenceChangedMessage = WM_APP + 3;

#endif  // RUNNER_APP_MESSAGES_H_
