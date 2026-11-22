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
      board.declareTypes([.string, concealed, transient], owner: nil)
      board.setString(text, forType: .string)
      board.setData(Data(), forType: concealed)
      board.setData(Data(), forType: transient)
      return board.changeCount
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
