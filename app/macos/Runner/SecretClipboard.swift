// The clipboard for secrets (factory/secret-clipboard): E5's "Copy code",
// a device's add code pasted rather than scanned. app/lib/people/
// secret_clipboard.dart is its Dart side; Fugo set the conditions.
//
// `copy` {text, seconds} writes [text] as plain text marked sensitive, and
// answers a token for that write: the pasteboard's changeCount. On the
// iPhone and iPad it expires after [seconds]; on the Mac it carries the
// nspasteboard.org concealed and transient types, so clipboard managers and
// their histories skip it. Neither is limited to this device: Universal
// Clipboard may carry it to the person's own others. `clear` {token}
// clears the clipboard only while it still holds that write (changeCount
// unchanged), so whatever the person copied since stays.
#if os(iOS)
  import UIKit
  import UniformTypeIdentifiers
  typealias Pasteboard = UIPasteboard
#else
  import AppKit
  typealias Pasteboard = NSPasteboard
#endif
#if canImport(Flutter)
  import Flutter
#elseif canImport(FlutterMacOS)
  import FlutterMacOS
#endif

enum SecretClipboard {
  static let channelName = "factory/secret-clipboard"

  /// The nspasteboard.org types clipboard managers honour.
  static let concealedType = "org.nspasteboard.ConcealedType"
  static let transientType = "org.nspasteboard.TransientType"

  /// On the Mac, a copy also carries when it stops working, in a type of
  /// its own whose name ends in that time (milliseconds since 1970), so
  /// [sweep] reads only the pasteboard's types, never its text (Fugo's
  /// ruling on row 181). iOS expires it itself (.expirationDate).
  static let expiresPrefix = "com.claktory.expires."

  #if canImport(Flutter) || canImport(FlutterMacOS)
  static func register(with messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: channelName, binaryMessenger: messenger)
    channel.setMethodCallHandler { call, result in
      let args = call.arguments as? [String: Any] ?? [:]
      switch call.method {
      case "copy":
        guard let text = args["text"] as? String else {
          result(FlutterError(code: "failed", message: "No text to copy.", details: nil))
          return
        }
        let seconds = (args["seconds"] as? NSNumber)?.doubleValue ?? 1800
        result(copy(text, seconds: seconds))
      case "clear":
        guard let token = (args["token"] as? NSNumber)?.intValue else {
          result(FlutterError(code: "failed", message: "No token.", details: nil))
          return
        }
        clear(token: token)
        result(nil)
      case "sweep":
        sweepAndWatch()
        result(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }
  #endif

  /// Writes [text], marked sensitive, and answers the write's token.
  static func copy(_ text: String, seconds: Double, on board: Pasteboard = .general) -> Int {
    #if os(iOS)
      board.setItems(
        [[UTType.utf8PlainText.identifier: text]],
        options: [.expirationDate: Date(timeIntervalSinceNow: max(1, seconds))])
      return board.changeCount
    #else
      board.clearContents()
      let concealed = NSPasteboard.PasteboardType(concealedType)
      let transient = NSPasteboard.PasteboardType(transientType)
      let ms = Int64((Date().timeIntervalSince1970 + max(1, seconds)) * 1000)
      let expires = NSPasteboard.PasteboardType(expiresPrefix + String(ms))
      board.declareTypes([.string, concealed, transient, expires], owner: nil)
      board.setString(text, forType: .string)
      board.setData(Data(), forType: concealed)
      board.setData(Data(), forType: transient)
      board.setData(Data(), forType: expires)
      return board.changeCount
    #endif
  }

  /// Clears the pasteboard when it holds a copy of ours that has stopped
  /// working, the app having quit or been away when its clear was due;
  /// anything else, the person's own copies included, stays. Reads only
  /// the types. Answers whether it cleared. Nothing on iOS.
  @discardableResult
  static func sweep(now: Date = Date(), on board: Pasteboard = .general) -> Bool {
    #if os(iOS)
      return false
    #else
      let nowMs = Int64(now.timeIntervalSince1970 * 1000)
      for type in board.types ?? [] where type.rawValue.hasPrefix(expiresPrefix) {
        guard let expires = Int64(type.rawValue.dropFirst(expiresPrefix.count)),
          expires <= nowMs
        else { continue }
        board.clearContents()
        return true
      }
      return false
    #endif
  }

  /// The time a copy of ours still on [board] stops working (ms since
  /// 1970), when that's still ahead; nil when there's none. Reads only
  /// the types.
  static func expiresAhead(now: Date = Date(), on board: Pasteboard = .general) -> Int64? {
    #if os(iOS)
      return nil
    #else
      let nowMs = Int64(now.timeIntervalSince1970 * 1000)
      return (board.types ?? [])
        .filter { $0.rawValue.hasPrefix(expiresPrefix) }
        .compactMap { Int64($0.rawValue.dropFirst(expiresPrefix.count)) }
        .filter { $0 > nowMs }
        .min()
    #endif
  }

  #if !os(iOS)
    /// The one sweep waiting for a copy's expiry, in this process.
    private static var watch: DispatchWorkItem?
  #endif

  /// [sweep], then, while a copy of ours still works, another at the
  /// moment it stops (Zucchero's row 185: a code copied before a relaunch,
  /// the app in front past its expiry). One waits at a time; each sweep
  /// replaces it.
  static func sweepAndWatch() {
    #if !os(iOS)
      sweep()
      watch?.cancel()
      watch = nil
      guard let expires = expiresAhead() else { return }
      let item = DispatchWorkItem { sweepAndWatch() }
      watch = item
      let delay = Double(expires) / 1000 - Date().timeIntervalSince1970 + 0.5
      DispatchQueue.main.asyncAfter(deadline: .now() + max(0.5, delay), execute: item)
    #endif
  }

  /// Clears the clipboard if it still holds the write [token] names.
  static func clear(token: Int, on board: Pasteboard = .general) {
    guard board.changeCount == token else { return }
    #if os(iOS)
      board.items = []
    #else
      board.clearContents()
    #endif
  }
}
