import Flutter
import SafariServices
import UIKit

/// Opening a website a friend shares over a pipe on this iPhone or iPad
/// (W6, #505). The app forwards the share's port to Claktory's machine
/// through the tunnel, so the page loads only while the app runs.
///
/// So the page opens in Safari's own view over the app
/// (SFSafariViewController), not in Safari: the app stays in front, its
/// connection and the forward with it, and Done comes straight back to
/// where the person was. It's presented over the app's screen, which stays
/// under it, so Flutter never hears the app went away.
///
/// factory/share-browser, as Android's (ShareBrowser.kt):
/// - `open {url, fallback}` → "in_app", or error `bad_url` / `no_browser` /
///   `no_localhost`. A share's own address opens, rebuilt from its checked
///   parts ([shareAddress]), when the system resolves it to this device
///   alone; else its fallback at 127.0.0.1, the same port ([fallbackAddress]).
///   The app calls it only from the person's tap on Open.
/// factory/share-browser/events: `{visible: Bool}` while the page is on
///   screen, and `{visible: true, loaded: false}` when its first load
///   failed (nothing answered at the forward's port).
final class ShareBrowser: NSObject, FlutterStreamHandler, SFSafariViewControllerDelegate {
  static let shared = ShareBrowser()

  private var sink: FlutterEventSink?
  private weak var shown: SFSafariViewController?
  private var visible = false

  static func register(with messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: "factory/share-browser", binaryMessenger: messenger)
    channel.setMethodCallHandler { call, result in
      switch call.method {
      case "open":
        let args = call.arguments as? [String: Any]
        shared.open(args?["url"] as? String, fallback: args?["fallback"] as? String, result)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
    FlutterEventChannel(name: "factory/share-browser/events", binaryMessenger: messenger)
      .setStreamHandler(shared)
  }

  private func open(_ given: String?, fallback: String?, _ result: @escaping FlutterResult) {
    guard let url = Self.shareAddress(given) else {
      result(FlutterError(
        code: "bad_url", message: "Only a shared website's own address opens here.", details: nil))
      return
    }
    // Safari asks the system for s-<id>.localhost, and before iOS 26 the
    // system asked the network's DNS server, which may not answer for it,
    // or answer with an address elsewhere (WebKit bug 160504). Asked here
    // first, the same way, off the main thread, at every Open (nothing
    // kept): only an answer that's this device's loopback, all of it,
    // opens the share's own address; anything else (no answer, an error,
    // a second, an address elsewhere) opens the fallback our Claktory made,
    // 127.0.0.1 at the same port, guarded by the same checks as Mac
    // Safari's (Fugo 38931); without one, it says so and nothing opens.
    let host = url.host ?? ""
    let fallbackURL = Self.fallbackAddress(fallback, port: url.port)
    DispatchQueue.global(qos: .userInitiated).async {
      let reachable = Self.resolvesToLoopback(host, within: Self.resolveWithin)
      DispatchQueue.main.async {
        if reachable {
          self.present(url, result)
        } else if let fallbackURL {
          self.present(fallbackURL, result)
        } else {
          result(FlutterError(
            code: "no_localhost",
            message: "This device can't open a shared website's address on this network.",
            details: nil))
        }
      }
    }
  }

  /// How long the system has to answer for a share's address.
  static let resolveWithin: TimeInterval = 1

  private func present(_ url: URL, _ result: @escaping FlutterResult) {
    guard let top = Self.presenter() else {
      result(FlutterError(code: "no_browser", message: "Nothing to show it over.", details: nil))
      return
    }
    // One page at a time: another Open replaces it.
    if let old = shown {
      old.dismiss(animated: false)
    }
    let configuration = SFSafariViewController.Configuration()
    configuration.entersReaderIfAvailable = false
    configuration.barCollapsingEnabled = true
    let page = SFSafariViewController(url: url, configuration: configuration)
    page.delegate = self
    page.dismissButtonStyle = .done
    // Over the app, which stays under it: the app isn't told it went
    // away, so its connection, and the share's forward, stay up.
    page.modalPresentationStyle = .overFullScreen
    shown = page
    top.present(page, animated: true)
    report(visible: true)
    result("in_app")
  }

  // MARK: SFSafariViewControllerDelegate

  func safariViewController(
    _ controller: SFSafariViewController, didCompleteInitialLoad didLoadSuccessfully: Bool
  ) {
    guard controller === shown, !didLoadSuccessfully else { return }
    sink?(["visible": true, "loaded": false])
  }

  func safariViewControllerDidFinish(_ controller: SFSafariViewController) {
    guard controller === shown else { return }
    shown = nil
    report(visible: false)
  }

  private func report(visible: Bool) {
    self.visible = visible
    sink?(["visible": visible])
  }

  // MARK: FlutterStreamHandler

  func onListen(withArguments arguments: Any?, eventSink events: @escaping FlutterEventSink)
    -> FlutterError?
  {
    sink = events
    events(["visible": visible])
    return nil
  }

  func onCancel(withArguments arguments: Any?) -> FlutterError? {
    sink = nil
    return nil
  }

  // MARK: Addresses

  /// [given] rebuilt from its parts if it's a shared website's address on
  /// this device: http, a share's own host (s-<id>.localhost), a port, no
  /// user; else nil. What opens is exactly what was checked.
  static func shareAddress(_ given: String?) -> URL? {
    guard let given, let parts = URLComponents(string: given) else { return nil }
    guard parts.scheme?.lowercased() == "http", parts.user == nil, parts.password == nil,
      let host = parts.host?.lowercased(), isShareHost(host),
      let port = parts.port, (1...65535).contains(port)
    else { return nil }
    var rebuilt = URLComponents()
    rebuilt.scheme = "http"
    rebuilt.host = host
    rebuilt.port = port
    rebuilt.percentEncodedPath = parts.percentEncodedPath.hasPrefix("/") ? parts.percentEncodedPath : "/"
    rebuilt.percentEncodedQuery = parts.percentEncodedQuery
    rebuilt.percentEncodedFragment = parts.percentEncodedFragment
    return rebuilt.url
  }

  /// [given] rebuilt from its parts if it's a share's fallback address on
  /// this device at [port], the share's own address's: http, exactly
  /// 127.0.0.1, that port, no user; else nil. Only opened once the share's
  /// own address didn't resolve to this device ([open]), never asked for.
  static func fallbackAddress(_ given: String?, port: Int?) -> URL? {
    guard let given, let port, let parts = URLComponents(string: given) else { return nil }
    guard parts.scheme?.lowercased() == "http", parts.user == nil, parts.password == nil,
      parts.host == "127.0.0.1", parts.port == port
    else { return nil }
    var rebuilt = URLComponents()
    rebuilt.scheme = "http"
    rebuilt.host = "127.0.0.1"
    rebuilt.port = port
    rebuilt.percentEncodedPath = parts.percentEncodedPath.hasPrefix("/") ? parts.percentEncodedPath : "/"
    rebuilt.percentEncodedQuery = parts.percentEncodedQuery
    rebuilt.percentEncodedFragment = parts.percentEncodedFragment
    return rebuilt.url
  }

  /// s-<id>.localhost: lowercase letters and digits, 1 to 64 of them.
  static func isShareHost(_ host: String) -> Bool {
    let suffix = ".localhost"
    guard host.hasPrefix("s-"), host.hasSuffix(suffix) else { return false }
    let id = host.dropFirst(2).dropLast(suffix.count)
    return (1...64).contains(id.count)
      && id.allSatisfy { ("a"..."z").contains($0) || ("0"..."9").contains($0) }
  }

  /// [resolvesToLoopback], false unless the system answers [within].
  static func resolvesToLoopback(_ host: String, within: TimeInterval) -> Bool {
    let answered = DispatchSemaphore(value: 0)
    let result = Answer()
    DispatchQueue.global(qos: .userInitiated).async {
      result.loopback = resolvesToLoopback(host)
      answered.signal()
    }
    guard answered.wait(timeout: .now() + within) == .success else { return false }
    return result.loopback
  }

  private final class Answer: @unchecked Sendable {
    var loopback = false
  }

  /// Whether the system resolves [host] to this device's own loopback
  /// (127.0.0.0/8 or ::1), and to nothing else: what Safari will reach.
  static func resolvesToLoopback(_ host: String) -> Bool {
    var hints = addrinfo()
    hints.ai_family = AF_UNSPEC
    hints.ai_socktype = SOCK_STREAM
    var found: UnsafeMutablePointer<addrinfo>?
    guard getaddrinfo(host, nil, &hints, &found) == 0, let first = found else { return false }
    defer { freeaddrinfo(first) }
    var any = false
    var next: UnsafeMutablePointer<addrinfo>? = first
    while let entry = next {
      next = entry.pointee.ai_next
      guard let address = entry.pointee.ai_addr else { continue }
      switch Int32(address.pointee.sa_family) {
      case AF_INET:
        let v4 = address.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { $0.pointee }
        guard UInt32(bigEndian: v4.sin_addr.s_addr) >> 24 == 127 else { return false }
      case AF_INET6:
        var v6 = address.withMemoryRebound(to: sockaddr_in6.self, capacity: 1) { $0.pointee }.sin6_addr
        var loopback = in6addr_loopback
        guard memcmp(&v6, &loopback, MemoryLayout<in6_addr>.size) == 0 else { return false }
      default:
        continue
      }
      any = true
    }
    return any
  }

  /// What's on screen, to present the page over.
  private static func presenter() -> UIViewController? {
    let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
    let scene = scenes.first { $0.activationState == .foregroundActive } ?? scenes.first
    var top = scene?.windows.first { $0.isKeyWindow }?.rootViewController
      ?? scene?.windows.first?.rootViewController
    while let presented = top?.presentedViewController { top = presented }
    return top
  }
}
