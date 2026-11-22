// Documents with ink on iPad (#209, ipad.md §2–§4): the native side of the
// ink bridge. Flutter embeds a surface per document pane ("claktory/ink",
// app/lib/ipad/ink_surface.dart) and talks to it on "claktory/ink/<id>";
// the app-wide "claktory/ink" channel keeps the render cache. The messages
// are the frozen fixtures in app/test/ipad/fixtures/bridge (README there).
//
// Pen motion never crosses the bridge: PencilKit draws here, on the UI
// thread, and Dart hears of finished changes only, coalesced, to save as
// revisions of an ink layer. docs/ipad-ink-spike.md says why each choice
// below was made.

import CryptoKit
import Flutter
import PencilKit
import UIKit

enum Ink {
  static let viewType = "claktory/ink"

  static func register(with registry: FlutterPluginRegistry) {
    guard let registrar = registry.registrar(forPlugin: "Ink") else { return }
    register(registrar: registrar)
  }

  static func register(registrar: FlutterPluginRegistrar) {
    let messenger = registrar.messenger()
    // Eager: Flutter holds the view's own recognizers until its arena
    // decides; Dart claims every touch on the surface at once, so the
    // arena decides for us without delay (spike: no measurable cost).
    registrar.register(
      InkFactory(messenger: messenger), withId: viewType,
      gestureRecognizersBlockingPolicy: FlutterPlatformViewGestureRecognizersBlockingPolicyEager)
    register(messenger: messenger)
  }

  /// The app-wide channel: the render cache (InkRenderCache).
  static func register(messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: viewType, binaryMessenger: messenger)
    channel.setMethodCallHandler { call, result in
      InkRenderCache.shared.handle(call, result)
    }
    appChannel = channel
  }

  private static var appChannel: FlutterMethodChannel?
}

/// Makes a surface per UiKitView: PaperView for a PDF (iOS 16+), InkView
/// for an image, an excerpt or a blank sketch.
final class InkFactory: NSObject, FlutterPlatformViewFactory {
  private let messenger: FlutterBinaryMessenger

  init(messenger: FlutterBinaryMessenger) {
    self.messenger = messenger
  }

  func createArgsCodec() -> FlutterMessageCodec & NSObjectProtocol {
    FlutterStandardMessageCodec.sharedInstance()
  }

  func create(withFrame frame: CGRect, viewIdentifier viewId: Int64, arguments args: Any?)
    -> FlutterPlatformView
  {
    let params = InkParams(args as? [String: Any] ?? [:])
    let channel = FlutterMethodChannel(
      name: "\(Ink.viewType)/\(viewId)", binaryMessenger: messenger)
    return Self.surface(frame: frame, params: params, channel: channel)
  }

  static func surface(frame: CGRect, params: InkParams, channel: InkEvents) -> InkSurface {
    switch params.surface {
    case "pdf":
      if #available(iOS 16.0, *) {
        return PaperView(frame: frame, params: params, channel: channel)
      }
      return UnavailableSurface(
        frame: frame, params: params, channel: channel,
        error: [
          "code": "os_too_old", "recoverable": false, "feature": "pdf_page_overlay",
          "min_os": "16.0", "os_version": UIDevice.current.systemVersion,
        ])
    default:
      return InkView(frame: frame, params: params, channel: channel)
    }
  }
}

/// Where a surface's events go and its commands come from: a Flutter
/// method channel, or a test's stand-in.
protocol InkEvents: AnyObject {
  func invokeMethod(_ method: String, arguments: Any?)
  func setMethodCallHandler(_ handler: FlutterMethodCallHandler?)
}

extension FlutterMethodChannel: InkEvents {}

/// A surface's creation params (bridge/create_*.json).
struct InkParams {
  let raw: [String: Any]
  let leafId: String
  let surface: String
  let source: [String: Any]
  let document: [String: Any]
  let layers: [[String: Any]]
  let drawingPolicy: PKCanvasViewDrawingPolicy
  let active: Bool
  let initialPage: Int?
  let paper: UIColor
  let style: UIUserInterfaceStyle

  init(_ raw: [String: Any]) {
    self.raw = raw
    leafId = raw["leaf_id"] as? String ?? ""
    surface = raw["surface"] as? String ?? "blank"
    source = raw["source"] as? [String: Any] ?? [:]
    document = raw["document"] as? [String: Any] ?? [:]
    layers = raw["layers"] as? [[String: Any]] ?? []
    switch raw["drawing_policy"] as? String {
    case "pencil_only": drawingPolicy = .pencilOnly
    case "any_input": drawingPolicy = .anyInput
    default: drawingPolicy = .default
    }
    active = raw["active"] as? Bool ?? true
    initialPage = (raw["initial"] as? [String: Any])?["page_1based"] as? Int
    let ink = raw["ink"] as? [String: Any] ?? [:]
    paper = UIColor(inkHex: ink["paper"] as? String) ?? .white
    // Ink keeps the colours it was drawn in whatever the app's theme:
    // PencilKit's dynamic colours would turn black ink white over a
    // dark-mode page (ipad.md §2).
    style = ink["appearance"] as? String == "dark" ? .dark : .light
  }

  var sourceSha256: String? { source["sha256"] as? String }
}

// MARK: - Helpers

extension UIColor {
  /// "#RRGGBBAA" (or "#RRGGBB"), as the fixtures write colours.
  convenience init?(inkHex hex: String?) {
    guard var s = hex, s.hasPrefix("#") else { return nil }
    s.removeFirst()
    if s.count == 6 { s += "FF" }
    guard s.count == 8, let v = UInt64(s, radix: 16) else { return nil }
    self.init(
      red: CGFloat((v >> 24) & 0xFF) / 255, green: CGFloat((v >> 16) & 0xFF) / 255,
      blue: CGFloat((v >> 8) & 0xFF) / 255, alpha: CGFloat(v & 0xFF) / 255)
  }

  var inkHex: String {
    var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
    getRed(&r, green: &g, blue: &b, alpha: &a)
    func byte(_ c: CGFloat) -> String {
      String(format: "%02X", Int((max(0, min(1, c)) * 255).rounded()))
    }
    return "#" + byte(r) + byte(g) + byte(b) + byte(a)
  }
}

enum InkHash {
  static func sha256(_ data: Data) -> String {
    SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
  }

  /// A file's SHA-256, read in chunks (a PDF may be 64 MiB).
  static func sha256(file url: URL) -> String? {
    guard let handle = try? FileHandle(forReadingFrom: url) else { return nil }
    defer { try? handle.close() }
    var hasher = SHA256()
    do {
      // nil or empty: the end of the file.
      while let chunk = try handle.read(upToCount: 1 << 20), !chunk.isEmpty {
        hasher.update(data: chunk)
      }
    } catch { return nil }
    return hasher.finalize().map { String(format: "%02x", $0) }.joined()
  }
}

/// A rect in 0..1 of its document, {x, y, w, h}.
enum InkRegion {
  static func json(_ r: CGRect) -> [String: Double] {
    func round5(_ v: CGFloat) -> Double { (Double(v) * 100_000).rounded() / 100_000 }
    return ["x": round5(r.minX), "y": round5(r.minY), "w": round5(r.width), "h": round5(r.height)]
  }

  static func rect(_ any: Any?) -> CGRect? {
    guard let m = any as? [String: Any],
      let x = (m["x"] as? NSNumber)?.doubleValue, let y = (m["y"] as? NSNumber)?.doubleValue,
      let w = (m["w"] as? NSNumber)?.doubleValue, let h = (m["h"] as? NSNumber)?.doubleValue
    else { return nil }
    return CGRect(x: x, y: y, width: w, height: h)
  }

  static func normalized(_ r: CGRect, in size: CGSize) -> CGRect {
    guard size.width > 0, size.height > 0 else { return .zero }
    let n = CGRect(
      x: r.minX / size.width, y: r.minY / size.height, width: r.width / size.width,
      height: r.height / size.height)
    return n.intersection(CGRect(x: 0, y: 0, width: 1, height: 1))
  }
}

extension PKDrawing {
  /// The content version a reader must understand (PencilKit 17+), else 1.
  var inkContentVersion: Int {
    if #available(iOS 17.0, *) { return requiredContentVersion.rawValue }
    return 1
  }

  static var inkMaxContentVersion: Int {
    if #available(iOS 17.0, *) { return PKContentVersion.latest.rawValue }
    return 1
  }
}

/// A canvas whose undo is its surface's own: Cmd-Z and Undo on one
/// document never undo another, nor a text field's typing (ipad.md §1).
final class InkCanvas: PKCanvasView {
  var surfaceUndo: UndoManager?
  override var undoManager: UndoManager? { surfaceUndo ?? super.undoManager }
}

/// The view a surface hands Flutter: reports size changes, so a stroke in
/// flight when the layout changes is reported rather than silently lost.
final class InkRootView: UIView {
  /// A size change after the first layout: a stroke in progress is lost.
  var onResize: ((CGSize) -> Void)?
  /// Every size change, the first included: to fit the content.
  var onLayout: ((CGSize) -> Void)?
  var onTraits: (() -> Void)?
  private var last = CGSize.zero

  override func layoutSubviews() {
    super.layoutSubviews()
    guard bounds.size != last else { return }
    let had = last
    last = bounds.size
    if had != .zero { onResize?(bounds.size) }
    onLayout?(bounds.size)
  }

  override func traitCollectionDidChange(_ previous: UITraitCollection?) {
    super.traitCollectionDidChange(previous)
    onTraits?()
  }
}

extension Optional {
  /// The value, or NSNull for the channel's null.
  var inkJSON: Any { map { $0 as Any } ?? NSNull() }
}
