#ifndef RUNNER_SIGHT_CHANNEL_H_
#define RUNNER_SIGHT_CHANNEL_H_

#include <flutter/binary_messenger.h>
#include <flutter/encodable_value.h>
#include <flutter/method_channel.h>

#include <memory>

#include "presence.h"

// factory/sight on Windows (lib/sight.dart; the Mac's MainFlutterWindow.swift):
// whether anyone can see the window, so nothing in it moves while no one
// can, and whether the person asked for less motion, which Flutter's
// Windows embedder doesn't pass on.
// - "state": {shown, reduceMotion} now;
// - "seen": the pointer is over the window, so it's shown whatever was said
//   last; the next change is told again;
// - "sight" (to the app) {shown, reduceMotion}: sent as either changes.
// shown and reduceMotion are presence.h's Shown() and ReduceMotion().
class SightChannel {
 public:
  SightChannel(flutter::BinaryMessenger* messenger, const Presence& presence);
  ~SightChannel();

  // Looks again, after presence.h said something may have changed.
  void Changed();

 private:
  flutter::EncodableValue State(bool shown, bool reduce_motion) const;

  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel_;
  const Presence& presence_;
  bool asked_ = false;
  bool said_shown_ = true;
  bool said_reduce_motion_ = false;
};

#endif  // RUNNER_SIGHT_CHANNEL_H_
