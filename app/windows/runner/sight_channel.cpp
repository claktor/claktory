#include "sight_channel.h"

#include <flutter/standard_method_codec.h>

#include <string>
#include <utility>

using flutter::EncodableMap;
using flutter::EncodableValue;

SightChannel::SightChannel(flutter::BinaryMessenger* messenger,
                           const Presence& presence)
    : channel_(std::make_unique<flutter::MethodChannel<EncodableValue>>(
          messenger, "factory/sight",
          &flutter::StandardMethodCodec::GetInstance())),
      presence_(presence) {
  channel_->SetMethodCallHandler([this](const auto& call, auto result) {
    const std::string& method = call.method_name();
    if (method == "state") {
      asked_ = true;
      said_shown_ = presence_.Shown();
      said_reduce_motion_ = presence_.ReduceMotion();
      result->Success(State(said_shown_, said_reduce_motion_));
      return;
    }
    if (method == "seen") {
      // The app now shows itself as seen; a later "not shown" is news again.
      said_shown_ = true;
      result->Success();
      return;
    }
    result->NotImplemented();
  });
}

SightChannel::~SightChannel() { channel_->SetMethodCallHandler(nullptr); }

EncodableValue SightChannel::State(bool shown, bool reduce_motion) const {
  return EncodableValue(EncodableMap{
      {EncodableValue("shown"), EncodableValue(shown)},
      {EncodableValue("reduceMotion"), EncodableValue(reduce_motion)},
  });
}

void SightChannel::Changed() {
  // Nothing is told before the app asks: it starts from "state".
  if (!asked_) return;
  bool shown = presence_.Shown();
  bool reduce_motion = presence_.ReduceMotion();
  if (shown == said_shown_ && reduce_motion == said_reduce_motion_) return;
  said_shown_ = shown;
  said_reduce_motion_ = reduce_motion;
  channel_->InvokeMethod("sight", std::make_unique<EncodableValue>(
                                      State(shown, reduce_motion)));
}
