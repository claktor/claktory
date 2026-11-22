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

  /// The app-wide channel: the render cache (InkRenderCache) and the
  /// journal of drawings not yet saved (InkJournal).
  static func register(messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: viewType, binaryMessenger: messenger)
    channel.setMethodCallHandler { call, result in
      if call.method.hasPrefix("journal") {
        InkJournal.shared.handle(call, result)
      } else {
        InkRenderCache.shared.handle(call, result)
      }
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
  /// "none": nothing draws; the document scrolls, zooms and follows links.
  let drawingOff: Bool
  let active: Bool
  let initialPage: Int?
  let paper: UIColor
  let appearance: String
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
    drawingOff = raw["drawing_policy"] as? String == "none"
    active = raw["active"] as? Bool ?? true
    initialPage = (raw["initial"] as? [String: Any])?["page_1based"] as? Int
    let ink = raw["ink"] as? [String: Any] ?? [:]
    paper = UIColor(inkHex: ink["paper"] as? String) ?? .white
    // Ink keeps the exact colours it was drawn in, whatever the system's
    // or the app's appearance: PencilKit's dynamic colours would turn
    // black ink white on a page that stays white (ipad.md §2, HIG "Help
    // people draw on top of existing content"). So surfaces are always
    // light; `appearance` is read and never makes ink dynamic.
    appearance = ink["appearance"] as? String ?? "light"
    style = .light
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

  /// Whether a touch draws at all. PencilKit enables its drawing recognizer
  /// again when the canvas joins a window, so it's applied then too.
  var draws = true {
    didSet { drawingGestureRecognizer.isEnabled = draws }
  }

  override func didMoveToWindow() {
    super.didMoveToWindow()
    drawingGestureRecognizer.isEnabled = draws
  }

  /// Cancels the stroke in progress: a recognizer turned off mid-gesture
  /// is cancelled, and PencilKit drops what it hadn't committed.
  func cancelStroke() {
    drawingGestureRecognizer.isEnabled = false
    drawingGestureRecognizer.isEnabled = draws
  }

  /// A find's matches on this page, in the canvas's own space, washed in
  /// the find yellow under the ink. Drawn here rather than by PDFKit's
  /// highlightedSelections, which on an iPhone drew them solid red and
  /// left them on the page once cleared (Zucchero's row 75).
  var found: [CGRect] = [] {
    didSet {
      guard found != oldValue else { return }
      if found.isEmpty {
        foundLayer?.removeFromSuperlayer()
        foundLayer = nil
        return
      }
      let layer = foundLayer ?? CAShapeLayer()
      if foundLayer == nil {
        layer.fillColor = Self.findYellow.cgColor
        layer.zPosition = -1
        self.layer.insertSublayer(layer, at: 0)
        foundLayer = layer
      }
      let path = CGMutablePath()
      for rect in found {
        path.addPath(
          UIBezierPath(roundedRect: rect.insetBy(dx: -1, dy: -1), cornerRadius: 2).cgPath)
      }
      layer.frame = CGRect(origin: .zero, size: bounds.size)
      layer.path = path
    }
  }
  private(set) var foundLayer: CAShapeLayer?

  /// The match a find has gone to, marked apart from the rest: deeper, and
  /// outlined.
  var current: CGRect? {
    didSet {
      guard current != oldValue else { return }
      guard let current else {
        currentLayer?.removeFromSuperlayer()
        currentLayer = nil
        return
      }
      let layer = currentLayer ?? CAShapeLayer()
      if currentLayer == nil {
        layer.fillColor = Self.currentYellow.cgColor
        layer.strokeColor = Self.currentEdge.cgColor
        layer.lineWidth = 1.5
        layer.zPosition = -1
        self.layer.insertSublayer(layer, at: 0)
        currentLayer = layer
      }
      layer.frame = CGRect(origin: .zero, size: bounds.size)
      layer.path = UIBezierPath(roundedRect: current.insetBy(dx: -2, dy: -2), cornerRadius: 3).cgPath
    }
  }
  private(set) var currentLayer: CAShapeLayer?

  static let currentYellow = UIColor(red: 1, green: 0xB8 / 255, blue: 0, alpha: 0.55)
  static let currentEdge = UIColor(red: 0xE5 / 255, green: 0x8E / 255, blue: 0, alpha: 1)

  /// The app's find yellow (find.dart), part way through.
  static let findYellow = UIColor(red: 1, green: 0xD6 / 255, blue: 0x0A / 255, alpha: 0.45)
}

/// The view a surface hands Flutter: reports size changes, so a stroke in
/// flight when the layout changes is reported rather than silently lost.
final class InkRootView: UIView {
  /// A size change after the first layout: a stroke in progress is lost.
  var onResize: ((CGSize) -> Void)?
  /// Every size change, the first included: to fit the content.
  var onLayout: ((CGSize) -> Void)?
  var onTraits: (() -> Void)?
  /// The surface's status for VoiceOver: a band along the top.
  weak var status: UIAccessibilityElement?
  private var last = CGSize.zero

  override func layoutSubviews() {
    super.layoutSubviews()
    status?.accessibilityFrameInContainerSpace = CGRect(
      x: 0, y: 0, width: bounds.width, height: min(44, bounds.height))
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
