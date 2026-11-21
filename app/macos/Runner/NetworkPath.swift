// The device's network path, for the app to reconnect the moment it
// changes (app/lib/network.dart) rather than after missed pings: Wi-Fi to
// cellular, a network coming back, the Mac waking onto another. On the Mac
// and the iPhone: the iPhone's Runner builds this same file.
//
// factory/network (events): while listened to, the path now, then one event
// whenever it changes, on the main thread: {satisfied: Bool, interfaces:
// ["wifi" | "cellular" | "wired" | "loopback" | "other", …] (the order the
// system prefers them), expensive: Bool, constrained: Bool}.

import Network

#if os(macOS)
  import FlutterMacOS
#else
  import Flutter
#endif

final class NetworkPath: NSObject, FlutterStreamHandler {
  /// Kept for the app's lifetime, once registered.
  private static var shared: NetworkPath?

  static func register(with messenger: FlutterBinaryMessenger) {
    shared = NetworkPath(messenger: messenger)
  }

  private let events: FlutterEventChannel
  private var monitor: NWPathMonitor?
  private let queue = DispatchQueue(label: "claktory.network-path")

  private init(messenger: FlutterBinaryMessenger) {
    events = FlutterEventChannel(name: "factory/network", binaryMessenger: messenger)
    super.init()
    events.setStreamHandler(self)
  }

  func onListen(withArguments arguments: Any?, eventSink: @escaping FlutterEventSink)
    -> FlutterError?
  {
    monitor?.cancel()
    let monitor = NWPathMonitor()
    // The first call is the path as it is now.
    monitor.pathUpdateHandler = { path in
      let event = NetworkPath.describe(path)
      DispatchQueue.main.async { eventSink(event) }
    }
    monitor.start(queue: queue)
    self.monitor = monitor
    return nil
  }

  func onCancel(withArguments arguments: Any?) -> FlutterError? {
    monitor?.cancel()
    monitor = nil
    return nil
  }

  /// [path] as the event says it.
  static func describe(_ path: NWPath) -> [String: Any] {
    [
      "satisfied": path.status == .satisfied,
      "interfaces": path.availableInterfaces.map { name(of: $0.type) },
      "expensive": path.isExpensive,
      "constrained": path.isConstrained,
    ]
  }

  static func name(of type: NWInterface.InterfaceType) -> String {
    switch type {
    case .wifi: return "wifi"
    case .cellular: return "cellular"
    case .wiredEthernet: return "wired"
    case .loopback: return "loopback"
    default: return "other"
    }
  }
}
