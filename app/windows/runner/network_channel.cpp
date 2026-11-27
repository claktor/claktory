// winsock2.h before anything that pulls in windows.h, which would otherwise
// bring the old winsock.h with it.
#include <winsock2.h>
#include <ws2ipdef.h>
#include <iphlpapi.h>
#include <netioapi.h>
#include <windows.h>

#include "network_channel.h"

#include <flutter/event_stream_handler_functions.h>
#include <flutter/standard_method_codec.h>

#include <algorithm>
#include <memory>
#include <string>
#include <vector>

#include "app_messages.h"
#include "utils.h"

using flutter::EncodableList;
using flutter::EncodableMap;
using flutter::EncodableValue;

namespace {

void WINAPI InterfaceChanged(PVOID context, PMIB_IPINTERFACE_ROW,
                             MIB_NOTIFICATION_TYPE) {
  ::PostMessageW(static_cast<HWND>(context), kNetworkChangedMessage, 0, 0);
}

void WINAPI HintChanged(PVOID context, NL_NETWORK_CONNECTIVITY_HINT) {
  ::PostMessageW(static_cast<HWND>(context), kNetworkChangedMessage, 0, 0);
}

const char* InterfaceName(IFTYPE type) {
  switch (type) {
    case IF_TYPE_IEEE80211:
      return "wifi";
    case IF_TYPE_ETHERNET_CSMACD:
      return "wired";
    case IF_TYPE_WWANPP:
    case IF_TYPE_WWANPP2:
      return "cellular";
    case IF_TYPE_SOFTWARE_LOOPBACK:
      return "loopback";
    default:
      // VPNs and tunnels (Tailscale's among them), PPP, virtual switches.
      return "other";
  }
}

struct Interface {
  std::string name;
  ULONG metric = 0;
  std::string network;
};

// The interfaces that are up and lead somewhere (they have a gateway), the
// one Windows sends through first at the front.
std::vector<Interface> UsableInterfaces() {
  std::vector<Interface> found;
  ULONG size = 16 * 1024;
  std::vector<unsigned char> buffer;
  ULONG flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST |
                GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER |
                GAA_FLAG_SKIP_FRIENDLY_NAME;
  ULONG result = ERROR_BUFFER_OVERFLOW;
  for (int attempt = 0; attempt < 3 && result == ERROR_BUFFER_OVERFLOW;
       ++attempt) {
    buffer.resize(size);
    result = ::GetAdaptersAddresses(
        AF_UNSPEC, flags, nullptr,
        reinterpret_cast<IP_ADAPTER_ADDRESSES*>(buffer.data()), &size);
  }
  if (result != NO_ERROR) return found;
  for (auto* adapter = reinterpret_cast<IP_ADAPTER_ADDRESSES*>(buffer.data());
       adapter != nullptr; adapter = adapter->Next) {
    if (adapter->OperStatus != IfOperStatusUp) continue;
    if (adapter->FirstGatewayAddress == nullptr) continue;
    Interface usable;
    usable.name = InterfaceName(adapter->IfType);
    // Lower first; an IPv6-only interface by its IPv6 metric.
    usable.metric = adapter->Ipv4Enabled ? adapter->Ipv4Metric
                                         : adapter->Ipv6Metric;
    // Which network: the interface, and the address it has on it. A new
    // address (another Wi-Fi, a new lease) is another network, where open
    // connections may be dead.
    usable.network = std::to_string(adapter->Luid.Value);
    for (auto* address = adapter->FirstUnicastAddress; address != nullptr;
         address = address->Next) {
      if (address->Address.lpSockaddr == nullptr ||
          address->Address.lpSockaddr->sa_family != AF_INET) {
        continue;
      }
      const auto* ipv4 =
          reinterpret_cast<const sockaddr_in*>(address->Address.lpSockaddr);
      usable.network += ":" + std::to_string(ipv4->sin_addr.S_un.S_addr);
      break;
    }
    found.push_back(std::move(usable));
  }
  std::stable_sort(found.begin(), found.end(),
                   [](const Interface& a, const Interface& b) {
                     return a.metric < b.metric;
                   });
  return found;
}

EncodableValue Describe() {
  std::vector<Interface> interfaces = UsableInterfaces();
  NL_NETWORK_CONNECTIVITY_HINT hint{};
  bool hinted = ::GetNetworkConnectivityHint(&hint) == NO_ERROR;

  bool satisfied;
  if (hinted && hint.ConnectivityLevel != NetworkConnectivityLevelHintUnknown) {
    // Reaching this network is enough: a Claktory on it needs no internet.
    satisfied =
        hint.ConnectivityLevel == NetworkConnectivityLevelHintLocalAccess ||
        hint.ConnectivityLevel == NetworkConnectivityLevelHintInternetAccess ||
        hint.ConnectivityLevel ==
            NetworkConnectivityLevelHintConstrainedInternetAccess;
  } else {
    satisfied = !interfaces.empty();
  }
  bool expensive =
      hinted && (hint.ConnectivityCost == NetworkConnectivityCostHintFixed ||
                 hint.ConnectivityCost == NetworkConnectivityCostHintVariable ||
                 hint.Roaming);
  bool constrained = hinted && hint.OverDataLimit;

  EncodableList names;
  for (const auto& each : interfaces) names.emplace_back(each.name);
  EncodableMap event;
  event[EncodableValue("satisfied")] = EncodableValue(satisfied);
  event[EncodableValue("interfaces")] = EncodableValue(std::move(names));
  event[EncodableValue("expensive")] = EncodableValue(expensive);
  event[EncodableValue("constrained")] = EncodableValue(constrained);
  if (satisfied && !interfaces.empty()) {
    event[EncodableValue("network")] = EncodableValue(interfaces.front().network);
  }
  return EncodableValue(std::move(event));
}

}  // namespace

NetworkChannel::NetworkChannel(flutter::BinaryMessenger* messenger, HWND window)
    : channel_(std::make_unique<flutter::EventChannel<EncodableValue>>(
          messenger, "factory/network",
          &flutter::StandardMethodCodec::GetInstance())),
      window_(window) {
  channel_->SetStreamHandler(
      std::make_unique<flutter::StreamHandlerFunctions<EncodableValue>>(
          [this](const EncodableValue*,
                 std::unique_ptr<flutter::EventSink<EncodableValue>>&& events)
              -> std::unique_ptr<flutter::StreamHandlerError<EncodableValue>> {
            sink_ = std::move(events);
            last_ = EncodableValue();
            // The first event is the network as it is now.
            Changed();
            Start();
            return nullptr;
          },
          [this](const EncodableValue*)
              -> std::unique_ptr<flutter::StreamHandlerError<EncodableValue>> {
            Stop();
            sink_ = nullptr;
            return nullptr;
          }));
}

NetworkChannel::~NetworkChannel() { Stop(); }

void NetworkChannel::Start() {
  if (interface_notification_ == nullptr) {
    DWORD done = ::NotifyIpInterfaceChange(AF_UNSPEC, InterfaceChanged, window_,
                                           FALSE, &interface_notification_);
    if (done != NO_ERROR) {
      interface_notification_ = nullptr;
      Log("network changes won't be heard: " + Win32ErrorMessage(done));
    }
  }
  if (hint_notification_ == nullptr) {
    DWORD done = ::NotifyNetworkConnectivityHintChange(
        HintChanged, window_, FALSE, &hint_notification_);
    if (done != NO_ERROR) hint_notification_ = nullptr;
  }
}

void NetworkChannel::Stop() {
  // Waits for a callback running now to return; never called from one.
  if (interface_notification_ != nullptr) {
    ::CancelMibChangeNotify2(interface_notification_);
    interface_notification_ = nullptr;
  }
  if (hint_notification_ != nullptr) {
    ::CancelMibChangeNotify2(hint_notification_);
    hint_notification_ = nullptr;
  }
}

void NetworkChannel::Changed() {
  if (!sink_) return;
  EncodableValue now = Describe();
  if (now == last_) return;
  last_ = now;
  sink_->Success(now);
}
