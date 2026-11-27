#ifndef RUNNER_DEVICE_CHANNEL_H_
#define RUNNER_DEVICE_CHANNEL_H_

#include <flutter/binary_messenger.h>
#include <flutter/encodable_value.h>
#include <flutter/method_channel.h>

#include <memory>

#include "core/connection_store.h"

// factory/device on Windows: what this device keeps for the app, as the Mac's
// MainFlutterWindow.swift does on the same channel.
// - connections / setConnections / resetConnections: the connections
//   (lib/connections.dart's DeviceConnectionStorage), in user DPAPI
//   (file_store.h), failing with the store's codes;
// - drafts / setDrafts: unsent drafts by agent (lib/drafts.dart), a map of
//   strings, in drafts.json.
// - cacheFolder: where the viewers keep copies of files (Cache in the app's
//   private folder); downloadsFolder: the person's Downloads, wherever
//   they moved it; showFiles (paths): Explorer on their folder, with them
//   selected (Show in Explorer, the Mac's Show in Finder).
// - appToken (path): this PC's Claktory's app-token, read from that file
//   sharing delete (the backend replaces it at every start); null when
//   there's none, app_token_unreadable when it can't be read or isn't one.
// Everything else the channel carries elsewhere answers "not implemented", so
// the Dart side's MissingPluginException path runs, as on a platform without
// it: qrCode (no code is shown or read under authority B, windows.md section
// 5), webAuth (desktop sign-in uses the loopback receiver), shareFiles (Save
// to Downloads first), defaultBrowser (only Safari needs it).
class DeviceChannel {
 public:
  explicit DeviceChannel(flutter::BinaryMessenger* messenger);

 private:
  void Handle(const flutter::MethodCall<flutter::EncodableValue>& call,
              std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
                  result);

  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel_;
  std::unique_ptr<claktory::ConnectionStore> connections_;
};

#endif  // RUNNER_DEVICE_CHANNEL_H_
