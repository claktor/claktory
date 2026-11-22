// Notifications of the system for what a Claktory's backend says the person
// should hear of (#273 phase A; app/lib/notifications.dart decides what to
// post), and whether the person is using the app, so the backend notifies
// no one while they are.
//
// factory/notifications:
// - "listen" (from the app): answers whether the person is using the app
//   now, and starts the calls back, any click that came before first.
// - "post" (from the app) {id, title, body, thread, urgent, target}: shows
//   it; one with an id already shown replaces it. The system asks the
//   person the first time.
// - "permission" (from the app): whether macOS lets Claktory notify,
//   "not_determined", "denied" or "allowed"; "ask" asks the person (once:
//   macOS answers at once after that), then answers the same.
// - "openSettings" (from the app): System Settings → Notifications.
// - "using" (to the app) Bool: the app is active, its window key and on
//   screen, and the screen awake and unlocked; sent as it changes.
// - "clicked" (to the app) target: a notification was clicked; the window
//   comes forward first.

import Cocoa
import FlutterMacOS
import UserNotifications

final class Notifications: NSObject, UNUserNotificationCenterDelegate {
  /// Kept for the app's lifetime, once registered.
  private static var shared: Notifications?

  static func register(with messenger: FlutterBinaryMessenger, window: NSWindow) {
    let notifications = Notifications(messenger: messenger, window: window)
    shared = notifications
    // Set at launch, so a click that opened the app reaches it.
    UNUserNotificationCenter.current().delegate = notifications
  }

  private let channel: FlutterMethodChannel
  private weak var window: NSWindow?
  private var listening = false
  private var clicks: [Any] = []
  private var asleep = false
  private var locked = false
  private var said: Bool?

  private init(messenger: FlutterBinaryMessenger, window: NSWindow) {
    channel = FlutterMethodChannel(name: "factory/notifications", binaryMessenger: messenger)
    self.window = window
    super.init()
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else { return result(nil) }
      switch call.method {
      case "listen":
        self.listening = true
        self.said = self.using
        result(self.using)
        for click in self.clicks {
          self.channel.invokeMethod("clicked", arguments: click)
        }
        self.clicks.removeAll()
      case "post":
        self.post(call.arguments as? [String: Any] ?? [:])
        result(nil)
      case "permission":
        Notifications.permission(result)
      case "ask":
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) {
          _, _ in Notifications.permission(result)
        }
      case "openSettings":
        // System Settings → Notifications, at Claktory when it can say so.
        let id = Bundle.main.bundleIdentifier ?? ""
        let url =
          URL(string: "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=\(id)")
          ?? URL(string: "x-apple.systempreferences:com.apple.Notifications-Settings.extension")!
        NSWorkspace.shared.open(url)
        result(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
    let center = NotificationCenter.default
    for name in [NSApplication.didBecomeActiveNotification, NSApplication.didResignActiveNotification] {
      center.addObserver(self, selector: #selector(changed), name: name, object: nil)
    }
    for name in [
      NSWindow.didBecomeKeyNotification, NSWindow.didResignKeyNotification,
      NSWindow.didChangeOcclusionStateNotification, NSWindow.didMiniaturizeNotification,
      NSWindow.didDeminiaturizeNotification,
    ] {
      center.addObserver(self, selector: #selector(changed), name: name, object: window)
    }
    let workspace = NSWorkspace.shared.notificationCenter
    workspace.addObserver(
      self, selector: #selector(slept), name: NSWorkspace.screensDidSleepNotification, object: nil)
    workspace.addObserver(
      self, selector: #selector(slept), name: NSWorkspace.willSleepNotification, object: nil)
    workspace.addObserver(
      self, selector: #selector(slept), name: NSWorkspace.sessionDidResignActiveNotification,
      object: nil)
    workspace.addObserver(
      self, selector: #selector(woke), name: NSWorkspace.screensDidWakeNotification, object: nil)
    workspace.addObserver(
      self, selector: #selector(woke), name: NSWorkspace.didWakeNotification, object: nil)
    workspace.addObserver(
      self, selector: #selector(woke), name: NSWorkspace.sessionDidBecomeActiveNotification,
      object: nil)
    let distributed = DistributedNotificationCenter.default()
    distributed.addObserver(
      self, selector: #selector(screenLocked),
      name: NSNotification.Name("com.apple.screenIsLocked"), object: nil)
    distributed.addObserver(
      self, selector: #selector(screenUnlocked),
      name: NSNotification.Name("com.apple.screenIsUnlocked"), object: nil)
  }

  /// The person is using the app: active, its window key and on screen,
  /// and the screen awake and unlocked.
  private var using: Bool {
    guard let window, !asleep, !locked else { return false }
    return NSApp.isActive && window.isKeyWindow && !window.isMiniaturized
      && window.occlusionState.contains(.visible)
  }

  @objc private func changed() {
    let now = using
    guard listening, now != said else { return }
    said = now
    channel.invokeMethod("using", arguments: now)
  }

  @objc private func slept() {
    asleep = true
    changed()
  }

  @objc private func woke() {
    asleep = false
    changed()
  }

  @objc private func screenLocked() {
    locked = true
    changed()
  }

  @objc private func screenUnlocked() {
    locked = false
    changed()
  }

  /// Whether macOS lets Claktory notify: "not_determined", "denied" or
  /// "allowed", answered on the main thread.
  private static func permission(_ result: @escaping FlutterResult) {
    UNUserNotificationCenter.current().getNotificationSettings { settings in
      let answer: String
      switch settings.authorizationStatus {
      case .notDetermined: answer = "not_determined"
      case .denied: answer = "denied"
      default: answer = "allowed"
      }
      DispatchQueue.main.async { result(answer) }
    }
  }

  private func post(_ notification: [String: Any]) {
    let center = UNUserNotificationCenter.current()
    center.requestAuthorization(options: [.alert, .sound]) { granted, _ in
      guard granted else { return }
      let content = UNMutableNotificationContent()
      content.title = notification["title"] as? String ?? "Claktory"
      content.body = notification["body"] as? String ?? ""
      content.threadIdentifier = notification["thread"] as? String ?? ""
      content.sound = .default
      if notification["urgent"] as? Bool ?? false {
        content.interruptionLevel = .timeSensitive
      }
      if let target = notification["target"] as? [String: Any] {
        content.userInfo = ["target": target]
      }
      let id = notification["id"] as? String ?? UUID().uuidString
      center.add(UNNotificationRequest(identifier: id, content: content, trigger: nil))
    }
  }

  // MARK: UNUserNotificationCenterDelegate

  /// The app is in front but the person isn't at this Claktory (the app
  /// posts only then): shown all the same.
  func userNotificationCenter(
    _ center: UNUserNotificationCenter, willPresent notification: UNNotification,
    withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
  ) {
    completionHandler([.banner, .list, .sound])
  }

  func userNotificationCenter(
    _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
    withCompletionHandler completionHandler: @escaping () -> Void
  ) {
    defer { completionHandler() }
    guard response.actionIdentifier == UNNotificationDefaultActionIdentifier else { return }
    let target = response.notification.request.content.userInfo["target"] ?? [String: Any]()
    DispatchQueue.main.async { [weak self] in
      guard let self else { return }
      NSApp.activate(ignoringOtherApps: true)
      if let window = self.window {
        if window.isMiniaturized { window.deminiaturize(nil) }
        window.makeKeyAndOrderFront(nil)
      }
      if self.listening {
        self.channel.invokeMethod("clicked", arguments: target)
      } else {
        self.clicks.append(target)
      }
    }
  }
}
