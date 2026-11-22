// Notifications on the iPhone (#273).
//
// factory/notifications, as on the Mac (app/macos/Runner/Notifications.swift;
// app/lib/notifications.dart drives it):
// - "listen" (from the app): answers whether the person is using the app,
//   and starts the calls back, any tap that came before first.
// - "post" (from the app) {id, title, body, thread, urgent, target}: shown
//   only while the app is in front (a Claktory other than the one on screen
//   said it). In the background its push shows instead, on the lock screen.
// - "using" (to the app) Bool: the app is in front; sent as it changes.
// - "clicked" (to the app) target: a notification was tapped.
//
// factory/push, this iPhone's registration for pushes (app/lib/mobile/push.dart):
// - "state" → {permission, token, platform}: whether the person allows
//   notifications ("not_determined", "denied", "allowed"), this iPhone's
//   push token (hex; empty until Apple gives one), and "apns" or, for a
//   build for development, "apns_sandbox".
// - "ask" → permission: asks the person, the first time; allowed, it asks
//   Apple for the token, which comes as "token" {token, platform}.
// - "registration" {claktory} → {handle, token, relay, platform, registered,
//   key} or nil: what this iPhone registered for that Claktory.
// - "register" {claktory, handle, token, relay, platform, newKey} → key: keeps
//   the relay's handle, and the key (a new one when there's none, or with
//   newKey), which the app hands to that Claktory.
// - "kept" → [claktory]: the Claktories a key is kept for (never the keys).
// - "forget" {claktory}: both gone.
//
// A tap opens what it's about only when the app itself posted it, or when
// its push opens with a key kept here and was shown (Push.swift).

import Flutter
import UIKit
import UserNotifications

final class Notifications: NSObject, UNUserNotificationCenterDelegate {
  static let shared = Notifications()

  private var channel: FlutterMethodChannel?
  private var pushChannel: FlutterMethodChannel?
  private var listening = false
  private var taps: [[String: Any]] = []
  private var said: Bool?
  private var token = ""

  var keys: PushKeyStore = KeychainPushKeys()
  var registrations = PushRegistrations()

  #if DEBUG
    /// A build for development: its pushes come through Apple's sandbox.
    static let platform = "apns_sandbox"
  #else
    static let platform = "apns"
  #endif

  /// At launch, before it finishes: a tap that opened the app reaches it.
  func launch() {
    UNUserNotificationCenter.current().delegate = self
    let center = NotificationCenter.default
    for name in [
      UIApplication.didBecomeActiveNotification, UIApplication.willResignActiveNotification,
      UIApplication.didEnterBackgroundNotification,
    ] {
      center.addObserver(self, selector: #selector(changed), name: name, object: nil)
    }
    // A token can change: asked for at every launch once allowed.
    UNUserNotificationCenter.current().getNotificationSettings { settings in
      guard Self.permission(settings.authorizationStatus) == "allowed" else { return }
      DispatchQueue.main.async { UIApplication.shared.registerForRemoteNotifications() }
    }
  }

  func register(with messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: "factory/notifications", binaryMessenger: messenger)
    self.channel = channel
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else { return result(nil) }
      switch call.method {
      case "listen":
        self.listening = true
        self.said = self.using
        result(self.using)
        for tap in self.taps { channel.invokeMethod("clicked", arguments: tap) }
        self.taps.removeAll()
      case "post":
        self.post(call.arguments as? [String: Any] ?? [:])
        result(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
    let push = FlutterMethodChannel(name: "factory/push", binaryMessenger: messenger)
    pushChannel = push
    push.setMethodCallHandler { [weak self] call, result in
      guard let self else { return result(nil) }
      let arguments = call.arguments as? [String: Any] ?? [:]
      let claktory = arguments["claktory"] as? String ?? ""
      switch call.method {
      case "state":
        UNUserNotificationCenter.current().getNotificationSettings { settings in
          let permission = Self.permission(settings.authorizationStatus)
          DispatchQueue.main.async {
            result(["permission": permission, "token": self.token, "platform": Self.platform])
          }
        }
      case "ask":
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .badge, .sound]) {
          granted, _ in
          DispatchQueue.main.async {
            if granted { UIApplication.shared.registerForRemoteNotifications() }
            result(granted ? "allowed" : "denied")
          }
        }
      case "registration":
        guard !claktory.isEmpty, let registration = self.registrations.read(claktory),
          let key = self.keys.all()[claktory]
        else { return result(nil) }
        var answer = registration.map
        answer["key"] = FlutterStandardTypedData(bytes: key.key)
        result(answer)
      case "register":
        guard !claktory.isEmpty, let registration = PushRegistration(arguments) else {
          return result(FlutterError(code: "arguments", message: "No registration.", details: nil))
        }
        var key = self.keys.all()[claktory]
        if key == nil || arguments["newKey"] as? Bool == true {
          guard let made = PushKey.random() else {
            return result(FlutterError(code: "random", message: "No key could be made.", details: nil))
          }
          key = made
        }
        guard let key, self.keys.save(key, for: claktory),
          self.registrations.save(registration, for: claktory)
        else {
          return result(
            FlutterError(code: "keychain", message: "The keychain didn't take it.", details: nil))
        }
        result(FlutterStandardTypedData(bytes: key.key))
      case "kept":
        result(Array(self.keys.all().keys).sorted())
      case "forget":
        if !claktory.isEmpty {
          self.keys.remove(claktory)
          self.registrations.remove(claktory)
        }
        result(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }

  /// Apple gave this iPhone its push token.
  func received(token data: Data) {
    token = data.map { String(format: "%02x", $0) }.joined()
    pushChannel?.invokeMethod("token", arguments: ["token": token, "platform": Self.platform])
  }

  static func permission(_ status: UNAuthorizationStatus) -> String {
    switch status {
    case .notDetermined: return "not_determined"
    case .denied: return "denied"
    default: return "allowed"
    }
  }

  /// The person is using the app: it's in front.
  private var using: Bool { UIApplication.shared.applicationState == .active }

  @objc private func changed() {
    let now = using
    // Back in front, what was waiting is in view: the badge goes.
    if now {
      if #available(iOS 16.0, *) {
        UNUserNotificationCenter.current().setBadgeCount(0)
      } else {
        UIApplication.shared.applicationIconBadgeNumber = 0
      }
    }
    guard listening, now != said else { return }
    said = now
    channel?.invokeMethod("using", arguments: now)
  }

  /// Shown only while the app is in front: in the background, the push
  /// that Claktory sends shows instead.
  private func post(_ notification: [String: Any]) {
    guard using else { return }
    let center = UNUserNotificationCenter.current()
    center.getNotificationSettings { settings in
      guard Self.permission(settings.authorizationStatus) == "allowed" else { return }
      let content = UNMutableNotificationContent()
      content.title = notification["title"] as? String ?? "Claktory"
      content.body = notification["body"] as? String ?? ""
      content.threadIdentifier = notification["thread"] as? String ?? ""
      content.sound = .default
      if notification["urgent"] as? Bool ?? false { content.interruptionLevel = .timeSensitive }
      if let target = notification["target"] as? [String: Any] {
        content.userInfo = ["target": target]
      }
      let id = notification["id"] as? String ?? UUID().uuidString
      center.add(UNNotificationRequest(identifier: id, content: content, trigger: nil))
    }
  }

  // MARK: UNUserNotificationCenterDelegate

  /// In front: what the app posted shows; a push doesn't, since the app
  /// already has what it says from its Claktory.
  func userNotificationCenter(
    _ center: UNUserNotificationCenter, willPresent notification: UNNotification,
    withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
  ) {
    if notification.request.trigger is UNPushNotificationTrigger { return completionHandler([]) }
    completionHandler([.banner, .list, .sound])
  }

  func userNotificationCenter(
    _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
    withCompletionHandler completionHandler: @escaping () -> Void
  ) {
    defer { completionHandler() }
    guard response.actionIdentifier == UNNotificationDefaultActionIdentifier,
      let target = Self.target(of: response.notification.request, keys: keys)
    else { return }
    DispatchQueue.main.async { [weak self] in
      guard let self else { return }
      if self.listening {
        self.channel?.invokeMethod("clicked", arguments: target)
      } else {
        self.taps.append(target)
      }
    }
  }

  /// What a tap on `request` opens: what the app posted says; a push only
  /// what it opens to with `keys` (Push.swift), never what Apple's payload
  /// says besides.
  static func target(of request: UNNotificationRequest, keys: PushKeyStore) -> [String: Any]? {
    if request.trigger is UNPushNotificationTrigger {
      return PushOpener.tapped(request.content.userInfo["c"], keys: keys.all())
    }
    return request.content.userInfo["target"] as? [String: Any]
  }
}

extension PushKey {
  /// A new key, from the system's random numbers.
  static func random() -> PushKey? {
    var bytes = [UInt8](repeating: 0, count: 32)
    guard SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess else {
      return nil
    }
    return PushKey(key: Data(bytes))
  }
}

/// What this iPhone registered with the push relay for a Claktory.
struct PushRegistration: Codable, Equatable {
  /// The relay's handle for this iPhone, base64url.
  var handle: String
  /// The push token it was registered with, hex.
  var token: String
  /// The relay, as the Claktory's settings named it ("" for Claktory's own).
  var relay: String
  /// "apns" or "apns_sandbox".
  var platform: String
  /// When, in ms since 1970.
  var registered: Int64

  init?(_ arguments: [String: Any]) {
    guard let handle = arguments["handle"] as? String, !handle.isEmpty,
      let token = arguments["token"] as? String,
      let platform = arguments["platform"] as? String
    else { return nil }
    self.handle = handle
    self.token = token
    self.relay = arguments["relay"] as? String ?? ""
    self.platform = platform
    self.registered = (arguments["registered"] as? NSNumber)?.int64Value
      ?? Int64(Date().timeIntervalSince1970 * 1000)
  }

  var map: [String: Any] {
    ["handle": handle, "token": token, "relay": relay, "platform": platform, "registered": registered]
  }
}

/// The registrations, in the app's own keychain group: the notification
/// extension never sees the handles.
struct PushRegistrations {
  static let service = "com.claktory.notifications.registration"

  private func query(_ claktory: String) -> [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: Self.service,
      kSecAttrAccount as String: claktory,
    ]
  }

  func read(_ claktory: String) -> PushRegistration? {
    var lookup = query(claktory)
    lookup[kSecReturnData as String] = true
    lookup[kSecMatchLimit as String] = kSecMatchLimitOne
    var found: CFTypeRef?
    guard SecItemCopyMatching(lookup as CFDictionary, &found) == errSecSuccess,
      let data = found as? Data
    else { return nil }
    return try? JSONDecoder().decode(PushRegistration.self, from: data)
  }

  func save(_ registration: PushRegistration, for claktory: String) -> Bool {
    guard let data = try? JSONEncoder().encode(registration) else { return false }
    let update: [String: Any] = [
      kSecValueData as String: data,
      kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
    ]
    let status = SecItemUpdate(query(claktory) as CFDictionary, update as CFDictionary)
    if status == errSecSuccess { return true }
    guard status == errSecItemNotFound else { return false }
    var add = query(claktory)
    add.merge(update) { $1 }
    return SecItemAdd(add as CFDictionary, nil) == errSecSuccess
  }

  func remove(_ claktory: String) {
    SecItemDelete(query(claktory) as CFDictionary)
  }
}
