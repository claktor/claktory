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
