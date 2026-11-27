#ifndef RUNNER_NETWORK_CHANNEL_H_
#define RUNNER_NETWORK_CHANNEL_H_

#include <flutter/binary_messenger.h>
#include <flutter/encodable_value.h>
#include <flutter/event_channel.h>
#include <windows.h>

#include <memory>

// factory/network on Windows (lib/network.dart; the Mac's NetworkPath.swift):
// while the app listens, the network as it is now, then again whenever it
// may have changed, so the app reconnects at once rather than after missed
// pings (Wi-Fi to Ethernet, a network coming back, waking onto another).
// Each event: {satisfied, interfaces: ["wifi" | "cellular" | "wired" |
// "loopback" | "other", ...] in the order Windows prefers them (lowest route
// metric first), expensive (a metered connection), constrained (over its
// data limit), network: which network the first interface is on}.
//
// Windows tells of changes on its own threads (NotifyIpInterfaceChange, and
// NotifyNetworkConnectivityHintChange for metered and reachability changes);
// each is posted to the window (kNetworkChangedMessage) and read there.
class NetworkChannel {
 public:
  NetworkChannel(flutter::BinaryMessenger* messenger, HWND window);
  ~NetworkChannel();

  // The window got kNetworkChangedMessage: read the network, and tell the
  // app if it isn't what it was last told.
  void Changed();

 private:
  void Start();
  void Stop();

  std::unique_ptr<flutter::EventChannel<flutter::EncodableValue>> channel_;
  std::unique_ptr<flutter::EventSink<flutter::EncodableValue>> sink_;
  HWND window_;
  HANDLE interface_notification_ = nullptr;
  HANDLE hint_notification_ = nullptr;
  flutter::EncodableValue last_;
};

#endif  // RUNNER_NETWORK_CHANNEL_H_
