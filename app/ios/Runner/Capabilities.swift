// What this device is and can do, for the app to lay itself out by
// (app/lib, the iPad workbench: docs/ipad-workbench.md §3, hook 4), never by
// its window's size. On factory/device, "capabilities" answers:
//
//   {schema_version: 1,
//    idiom: "pad" | "phone" | "mac" | "unspecified",
//    ios_app_on_mac: Bool,
//    os_version: "27.2",
//    hardware_keyboard: Bool,
//    pencil: {hover, squeeze, double_tap},
//    pdf_page_overlay: {available, min_os: "16.0"},
//    stroke_recognizer: {available, min_os: "27.0"}}
//
// The pencil's are what this system on this kind of device can deliver
// (UIKit has the interaction, on an iPad), not whether a Pencil is paired,
// nor this model's own: iOS says neither. Only the keyboard changes during
// a run: factory/device/keyboard sends {hardware_keyboard: Bool} as one
// connects or disconnects.

import Flutter
import GameController
import UIKit

enum Capabilities {
  static let schemaVersion = 1

  static func current() -> [String: Any] {
    let device = UIDevice.current
    let pad = device.userInterfaceIdiom == .pad
    var hover = false
    var squeeze = false
    if #available(iOS 16.1, *) { hover = pad }
    if #available(iOS 17.5, *) { squeeze = pad }
    var overlay = false
    if #available(iOS 16.0, *) { overlay = true }
    var strokes = false
    if #available(iOS 27.0, *) { strokes = true }
    return [
      "schema_version": schemaVersion,
      "idiom": idiom(device.userInterfaceIdiom),
      "ios_app_on_mac": ProcessInfo.processInfo.isiOSAppOnMac,
      "os_version": device.systemVersion,
      "hardware_keyboard": hardwareKeyboard,
      "pencil": ["hover": hover, "squeeze": squeeze, "double_tap": pad],
      "pdf_page_overlay": ["available": overlay, "min_os": "16.0"],
      "stroke_recognizer": ["available": strokes, "min_os": "27.0"],
    ]
  }

  static func idiom(_ idiom: UIUserInterfaceIdiom) -> String {
    switch idiom {
    case .pad: return "pad"
    case .phone: return "phone"
    case .mac: return "mac"
    default: return "unspecified"
    }
  }

  static var hardwareKeyboard: Bool { GCKeyboard.coalesced != nil }
}

/// factory/device/keyboard: {hardware_keyboard} whenever a keyboard
/// connects or disconnects, while listened to.
final class KeyboardEvents: NSObject, FlutterStreamHandler {
  private static var shared: KeyboardEvents?

  static func register(with messenger: FlutterBinaryMessenger) {
    shared = KeyboardEvents(messenger: messenger)
  }

  private let events: FlutterEventChannel
  private var observers: [NSObjectProtocol] = []

  private init(messenger: FlutterBinaryMessenger) {
    events = FlutterEventChannel(name: "factory/device/keyboard", binaryMessenger: messenger)
    super.init()
    events.setStreamHandler(self)
  }

  func onListen(withArguments arguments: Any?, eventSink: @escaping FlutterEventSink)
    -> FlutterError?
  {
    stop()
    var last = Capabilities.hardwareKeyboard
    func say(_ now: Bool) {
      guard now != last else { return }
      last = now
      eventSink(["hardware_keyboard": now])
    }
    let center = NotificationCenter.default
    observers = [
      center.addObserver(forName: .GCKeyboardDidConnect, object: nil, queue: .main) { _ in
        say(true)
      },
      // The one going may still be the coalesced one as this arrives.
      center.addObserver(forName: .GCKeyboardDidDisconnect, object: nil, queue: .main) { note in
        let left = GCKeyboard.coalesced
        say(left != nil && left !== note.object as AnyObject?)
      },
    ]
    return nil
  }

  func onCancel(withArguments arguments: Any?) -> FlutterError? {
    stop()
    return nil
  }

  private func stop() {
    observers.forEach(NotificationCenter.default.removeObserver)
    observers = []
  }
}

/// The window's controls (the close, minimize and arrange buttons an iPad
/// window has at its top left) cover what the app draws there, and
/// Flutter's safe area doesn't know them. Two answers, as UIKit's corner
/// adaptation gives them:
///
/// - The root view's safe area grows on top to the controls' bottom (the
///   vertical adaptation), so a screen with no toolbar of its own (the
///   phone's routes in a narrow window, sheets) starts below them.
/// - factory/device/window_controls sends {leading, top}: how far right a
///   toolbar's leading items move to clear them (the horizontal
///   adaptation) and how much of the top padding is theirs, so the iPad's
///   workbench keeps its toolbar at the status bar, as Apple's apps do.
///
/// Nothing changes where there are no controls (an iPhone, iPadOS before
/// 26). An iPhone app run on an iPad gets the vertical adaptation too, so
/// its screens start below the controls.
final class WindowControls: UIView {
  private static var observer: NSObjectProtocol?
  private static var events: FlutterEventChannel?
  private static var handler: Events?
  private static var last: [String: Double] = ["leading": 0, "top": 0]

  /// Watches every window of every scene as it comes to the front, and
  /// says what it finds on factory/device/window_controls.
  static func start(with messenger: FlutterBinaryMessenger) {
    guard observer == nil else { return }
    let handler = Events()
    Self.handler = handler
    events = FlutterEventChannel(
      name: "factory/device/window_controls", binaryMessenger: messenger)
    events?.setStreamHandler(handler)
    observer = NotificationCenter.default.addObserver(
      forName: UIScene.didActivateNotification, object: nil, queue: .main
    ) { note in
      (note.object as? UIWindowScene)?.windows.forEach(watch)
    }
    for case let scene as UIWindowScene in UIApplication.shared.connectedScenes {
      scene.windows.forEach(watch)
    }
  }

  /// Sends the last word to a listener as it starts listening, then each
  /// change.
  private final class Events: NSObject, FlutterStreamHandler {
    var sink: FlutterEventSink?

    func onListen(withArguments arguments: Any?, eventSink: @escaping FlutterEventSink)
      -> FlutterError?
    {
      sink = eventSink
      eventSink(WindowControls.last)
      return nil
    }

    func onCancel(withArguments arguments: Any?) -> FlutterError? {
      sink = nil
      return nil
    }
  }

  /// On an iPad: as an iPad app, or as an iPhone app run on one (never
  /// on a Mac, whose iPhone apps may say "iPad" too: their controls are
  /// in the title bar).
  private static var onPad: Bool {
    if ProcessInfo.processInfo.isiOSAppOnMac { return false }
    return UIDevice.current.userInterfaceIdiom == .pad
      || UIDevice.current.model.hasPrefix("iPad")
  }

  /// Watches [window] from now on; once per window.
  private static func watch(_ window: UIWindow?) {
    // The app's own window only, never the keyboard's, an alert's or a
    // sheet's (UIKit's own root view controllers); and only on an iPad,
    // where windows have controls (an iPhone's landscape corners would
    // otherwise count as such). The iPhone app run on an iPad (the App
    // Store's, built for iPhone only) has the phone's idiom there but an
    // iPad's windows, controls and all.
    guard onPad, let window,
      window.rootViewController is FlutterViewController,
      !window.subviews.contains(where: { $0 is WindowControls })
    else { return }
    let watch = WindowControls(frame: window.bounds)
    watch.autoresizingMask = [.flexibleWidth, .flexibleHeight]
    watch.isHidden = true
    watch.isUserInteractionEnabled = false
    window.insertSubview(watch, at: 0)
  }

  /// A view pinned to one of the window's corner-adapted safe areas: its
  /// size changes whenever that area does (the app going full screen or
  /// into a window, the controls showing or hiding), which the window's
  /// own bounds and safe area don't always.
  private final class Probe: UIView {
    var changed: (() -> Void)?

    override func layoutSubviews() {
      super.layoutSubviews()
      changed?()
    }
  }

  private var probes: [Probe] = []

  override func didMoveToWindow() {
    super.didMoveToWindow()
    probes.forEach { $0.removeFromSuperview() }
    probes = []
    guard #available(iOS 26.0, *), let window else { return }
    for axis in [UIView.LayoutRegion.AdaptivityAxis.vertical, .horizontal] {
      let probe = Probe()
      probe.isHidden = true
      probe.isUserInteractionEnabled = false
      probe.translatesAutoresizingMaskIntoConstraints = false
      probe.changed = { [weak self] in self?.update() }
      addSubview(probe)
      let guide = window.layoutGuide(for: .safeArea(cornerAdaptation: axis))
      NSLayoutConstraint.activate([
        probe.topAnchor.constraint(equalTo: guide.topAnchor),
        probe.bottomAnchor.constraint(equalTo: guide.bottomAnchor),
        probe.leadingAnchor.constraint(equalTo: guide.leadingAnchor),
        probe.trailingAnchor.constraint(equalTo: guide.trailingAnchor),
      ])
      probes.append(probe)
    }
  }

  override func layoutSubviews() {
    super.layoutSubviews()
    update()
  }

  override func safeAreaInsetsDidChange() {
    super.safeAreaInsetsDidChange()
    update()
  }

  private func update() {
    guard #available(iOS 26.0, *), let window,
      let root = window.rootViewController
    else { return }
    // The window's own safe area isn't the root view controller's, so
    // what's added there never feeds back into this.
    let safe = window.safeAreaInsets
    let vertical = window.edgeInsets(for: .safeArea(cornerAdaptation: .vertical))
    let horizontal = window.edgeInsets(for: .safeArea(cornerAdaptation: .horizontal))
    let top = max(0, (vertical.top - safe.top).rounded())
    let leading = max(0, (horizontal.left - safe.left).rounded())
    if root.additionalSafeAreaInsets.top != top {
      root.additionalSafeAreaInsets.top = top
    }
    let now = ["leading": Double(leading), "top": Double(top)]
    guard now != Self.last else { return }
    Self.last = now
    Self.handler?.sink?(now)
  }
}
