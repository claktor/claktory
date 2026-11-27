// Notifications pushed to this iPhone while the app isn't connected (#273,
// notifications.md §3 in Research): each Claktory seals what it says with a
// key only this iPhone and that Claktory have (AES-256-GCM), the push relay
// sends it through Apple under a fixed line, "Claktory has something for
// you", and the notification extension (NotificationService.swift) opens it
// here and shows what it says.
//
// Built into both the app and its notification extension. The extension
// reads only the keys, from the keychain group the two share; the app's own
// connections (ConnectionStore.swift) stay in its own group.
//
// What doesn't open, or opens as something already shown or too old, keeps
// the generic line, and a tap on it only opens the app (Fugo's rules).

import CryptoKit
import Foundation
import Security
import UserNotifications

/// What a push said, opened.
struct PushNotice: Equatable {
  /// The Claktory's name, shown under the title.
  let claktory: String
  /// "needs_you", "alert", "reply" or "test".
  let kind: String
  let title: String
  let body: String
  /// How many things wait for the person, for the app's badge.
  let badge: Int?
  let urgent: Bool
  /// Its counter: each push to this iPhone from a Claktory counts one up.
  let ctr: UInt64
  /// What a tap opens.
  let target: PushTarget
}

/// What a tap on a push opens: an agent (at its Needs-you item, `seq`, or
/// its alert, `alert`), a pipe (at its held message), a chat, or Needs you
/// for a device waiting
/// to be allowed (`device`, #352 P4) or for a Needs-you push about no one
/// thing (`kind` "needs_you": what waited out quiet hours, #364 O3); none
/// of them for a test.
struct PushTarget: Equatable {
  var agent = ""
  var pipe = ""
  var chat = ""
  var seq: UInt64 = 0
  var device = ""
  /// The alert it's about: the app opens Needs you at it.
  var alert = ""
  /// The push's kind, "needs_you" or another.
  var kind = ""

  /// As the app's notifications take it (app/lib/notifications.dart's
  /// NotificationTarget): `claktory` is the id the app keeps the key by.
  func map(claktory: String) -> [String: Any] {
    [
      "claktory": claktory, "agentId": agent, "pipeId": pipe, "chatId": chat, "seq": Int(seq),
      "deviceId": device, "alertId": alert, "kind": kind,
    ]
  }
}

/// A Claktory's key as this iPhone keeps it, with what it has shown.
struct PushKey: Codable, Equatable {
  /// 32 bytes: AES-256-GCM.
  var key: Data
  /// The highest counter shown: a push that isn't past it is a replay.
  var ctr: UInt64 = 0
  /// The pushes shown lately (a digest of each one's sealed text), the
  /// newest last: only a tap on one of these opens what it's about.
  var shown: [String] = []

  /// How many shown pushes are kept.
  static let kept = 64

  mutating func show(_ ctr: UInt64, sealed: String) {
    self.ctr = max(self.ctr, ctr)
    shown.append(Self.digest(sealed))
    if shown.count > Self.kept { shown.removeFirst(shown.count - Self.kept) }
  }

  /// A push's sealed text, in short: half its SHA-256, base64.
  static func digest(_ sealed: String) -> String {
    Data(SHA256.hash(data: Data(sealed.utf8)).prefix(16)).base64EncodedString()
  }
}

enum PushOpener {
  /// How late past its ttl a push may still show: the clocks of the
  /// Claktory and this iPhone may differ a little.
  static let slack: TimeInterval = 5 * 60

  enum Outcome: Equatable {
    /// It opened with the key kept as `claktory`.
    case shows(claktory: String, notice: PushNotice)
    /// It shows the generic line, and why.
    case generic(Why)
  }

  enum Why: Equatable {
    /// No ciphertext, or no key of this iPhone's opens it.
    case unopened
    /// A version this app doesn't know, or not what a push holds.
    case unreadable
    /// Its counter isn't past the highest shown.
    case replayed
    /// Older than its ttl.
    case stale
  }

  /// Opens `sealed` (base64: nonce ‖ ciphertext ‖ tag) with the first of
  /// `keys` that does, and checks it's new and fresh at `now`.
  static func open(_ sealed: Any?, keys: [String: PushKey], now: Date) -> Outcome {
    guard let (claktory, json) = decrypt(sealed, keys: keys) else { return .generic(.unopened) }
    guard let notice = notice(json) else { return .generic(.unreadable) }
    guard let key = keys[claktory], notice.ctr > key.ctr else { return .generic(.replayed) }
    let sentAt = (json["sent_at"] as? NSNumber)?.doubleValue ?? 0
    let ttl = (json["ttl"] as? NSNumber)?.doubleValue ?? 0
    if sentAt / 1000 + ttl + slack < now.timeIntervalSince1970 { return .generic(.stale) }
    return .shows(claktory: claktory, notice: notice)
  }

  /// What a tap on a push with `sealed` opens: its target, only when the
  /// extension showed that very push (it kept its digest) and it opens with
  /// one of `keys`; nil otherwise, and the tap only opens the app. Whatever
  /// else the push carries isn't read: Apple's payload is the relay's to
  /// write.
  static func tapped(_ sealed: Any?, keys: [String: PushKey]) -> [String: Any]? {
    guard let text = sealed as? String,
      let (claktory, json) = decrypt(text, keys: keys), let notice = notice(json),
      keys[claktory]?.shown.contains(PushKey.digest(text)) == true
    else { return nil }
    return notice.target.map(claktory: claktory)
  }

  /// The plaintext of `sealed` and the key that opened it.
  static func decrypt(_ sealed: Any?, keys: [String: PushKey]) -> (String, [String: Any])? {
    guard let text = sealed as? String, let data = Data(base64Encoded: text),
      let box = try? AES.GCM.SealedBox(combined: data)
    else { return nil }
    for (claktory, key) in keys.sorted(by: { $0.key < $1.key }) where key.key.count == 32 {
      guard let plain = try? AES.GCM.open(box, using: SymmetricKey(data: key.key)) else { continue }
      // Padded with spaces, which JSON allows after the value.
      guard let json = try? JSONSerialization.jsonObject(with: plain) as? [String: Any] else {
        return nil
      }
      return (claktory, json)
    }
    return nil
  }

  /// The notice in a push's plaintext: version 1 only.
  static func notice(_ json: [String: Any]) -> PushNotice? {
    guard (json["v"] as? NSNumber)?.intValue == 1,
      let ctr = (json["ctr"] as? NSNumber)?.uint64Value
    else { return nil }
    func text(_ key: String, in object: [String: Any] = json) -> String {
      object[key] as? String ?? ""
    }
    let target = json["target"] as? [String: Any] ?? [:]
    return PushNotice(
      claktory: text("claktory"), kind: text("kind"), title: text("title"), body: text("body"),
      badge: (json["badge"] as? NSNumber)?.intValue, urgent: json["urgent"] as? Bool ?? false,
      ctr: ctr,
      target: PushTarget(
        agent: text("agent", in: target), pipe: text("pipe", in: target),
        chat: text("chat", in: target),
        seq: (target["seq"] as? NSNumber)?.uint64Value ?? 0,
        // Older Claktories send none: no device.
        device: text("device", in: target), alert: text("alert", in: target),
        kind: text("kind")))
  }
}

/// Where the keys are kept: the keychain group the app shares with its
/// notification extension, readable after the first unlock (pushes come
/// while the iPhone is locked) and never leaving this device. A test keeps
/// them in memory.
protocol PushKeyStore: AnyObject {
  func all() -> [String: PushKey]
  func save(_ key: PushKey, for claktory: String) -> Bool
  func remove(_ claktory: String)

  /// The keychain's answer to the last save: errSecSuccess, or why not.
  var lastStatus: OSStatus { get }
}

final class KeychainPushKeys: PushKeyStore {
  static let service = "com.claktory.notifications"

  /// The group, as Info.plist says it (ClaktoryPushKeychainGroup): the team's
  /// prefix and com.claktory.app.notifications, the prefix found here when
  /// the build left it out ([entitled]).
  let group: String?

  private(set) var lastStatus: OSStatus = errSecSuccess

  init(
    group: String? = Bundle.main.object(forInfoDictionaryKey: "ClaktoryPushKeychainGroup")
      as? String,
    defaultGroup: () -> String? = KeychainPushKeys.appDefaultGroup
  ) {
    self.group = Self.entitled(group, defaultGroup: defaultGroup)
  }

  /// [group] with its team prefix. A build archived unsigned (testflight.sh
  /// signs afterwards) leaves $(AppIdentifierPrefix) empty in Info.plist,
  /// and a group without the prefix is one the app isn't entitled to: every
  /// save fails (errSecMissingEntitlement), and v0.1.16 never registered
  /// for pushes. The prefix is then the one the keychain gives the app's
  /// own items ([defaultGroup]); without one, the group stays as it is and
  /// saving it fails, and says so ([lastStatus]), rather than keeping the
  /// keys where the notification extension can't read them. Nil when
  /// Info.plist names none (a test).
  static func entitled(_ group: String?, defaultGroup: () -> String?) -> String? {
    guard let group, !group.isEmpty, !group.hasPrefix("$(") else { return nil }
    if teamPrefix(of: group) != nil { return group }
    guard let prefix = defaultGroup().flatMap(teamPrefix(of:)) else { return group }
    return "\(prefix).\(group)"
  }

  /// A group's team prefix ("ABCDE12345" of "ABCDE12345.com.claktory.app"),
  /// or nil when it has none.
  static func teamPrefix(of group: String) -> String? {
    let first = group.prefix { $0 != "." }
    guard first.count == 10, first.count < group.count,
      first.allSatisfy({ $0.isASCII && ($0.isUppercase || $0.isNumber) })
    else { return nil }
    return String(first)
  }

  /// The access group the keychain puts this app's items in when none is
  /// named: its first, with the team's prefix. Read from an empty item of
  /// its own, made the first time.
  static func appDefaultGroup() -> String? {
    let probe: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "com.claktory.keychain-group",
      kSecAttrAccount as String: "probe",
      kSecReturnAttributes as String: true,
    ]
    var found: CFTypeRef?
    var status = SecItemCopyMatching(probe as CFDictionary, &found)
    if status == errSecItemNotFound {
      var add = probe
      add[kSecValueData as String] = Data()
      add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
      status = SecItemAdd(add as CFDictionary, &found)
    }
    guard status == errSecSuccess, let attributes = found as? [String: Any] else { return nil }
    return attributes[kSecAttrAccessGroup as String] as? String
  }

  private func query() -> [String: Any] {
    var query: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: Self.service,
    ]
    if let group { query[kSecAttrAccessGroup as String] = group }
    return query
  }

  func all() -> [String: PushKey] {
    var lookup = query()
    lookup[kSecReturnAttributes as String] = true
    lookup[kSecReturnData as String] = true
    lookup[kSecMatchLimit as String] = kSecMatchLimitAll
    var found: CFTypeRef?
    guard SecItemCopyMatching(lookup as CFDictionary, &found) == errSecSuccess,
      let items = found as? [[String: Any]]
    else { return [:] }
    var keys: [String: PushKey] = [:]
    for item in items {
      guard let account = item[kSecAttrAccount as String] as? String,
        let data = item[kSecValueData as String] as? Data,
        let key = try? JSONDecoder().decode(PushKey.self, from: data)
      else { continue }
      keys[account] = key
    }
    return keys
  }

  func save(_ key: PushKey, for claktory: String) -> Bool {
    guard let data = try? JSONEncoder().encode(key) else {
      lastStatus = errSecParam
      return false
    }
    var match = query()
    match[kSecAttrAccount as String] = claktory
    let update: [String: Any] = [
      kSecValueData as String: data,
      kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
    ]
    lastStatus = SecItemUpdate(match as CFDictionary, update as CFDictionary)
    if lastStatus == errSecItemNotFound {
      var add = match
      add.merge(update) { $1 }
      lastStatus = SecItemAdd(add as CFDictionary, nil)
    }
    return lastStatus == errSecSuccess
  }

  func remove(_ claktory: String) {
    var match = query()
    match[kSecAttrAccount as String] = claktory
    SecItemDelete(match as CFDictionary)
  }
}

final class MemoryPushKeys: PushKeyStore {
  var keys: [String: PushKey] = [:]
  var lastStatus: OSStatus = errSecSuccess

  func all() -> [String: PushKey] { keys }

  func save(_ key: PushKey, for claktory: String) -> Bool {
    keys[claktory] = key
    return true
  }

  func remove(_ claktory: String) { keys[claktory] = nil }
}

// What a push shows: built here so the app's tests reach it too.
extension PushOpener {
  /// What `content` shows once opened with the keys in `store`; the shown
  /// push's counter is kept, so the same push can't show twice.
  static func content(
    for content: UNNotificationContent, keys store: PushKeyStore, now: Date
  ) -> UNNotificationContent {
    let keys = store.all()
    guard case .shows(let claktory, let notice) = PushOpener.open(
      content.userInfo["c"], keys: keys, now: now),
      var key = keys[claktory]
    else { return generic(content) }
    guard let sealed = content.userInfo["c"] as? String else { return generic(content) }
    key.show(notice.ctr, sealed: sealed)
    // Not kept, it could show again: the generic line instead.
    guard store.save(key, for: claktory) else { return generic(content) }
    let shown = UNMutableNotificationContent()
    shown.title = notice.title.isEmpty ? "Claktory" : notice.title
    shown.subtitle = notice.claktory
    shown.body = notice.body
    shown.sound = content.sound
    shown.threadIdentifier = content.threadIdentifier
    // How loudly is the sealed notice's to say, not Apple's payload: the
    // relay writes that, and an urgent push breaks through Focus.
    shown.interruptionLevel = notice.urgent ? .timeSensitive : .active
    if let badge = notice.badge { shown.badge = NSNumber(value: badge) }
    // The sealed text stays, for the app to open again on a tap
    // (PushOpener.tapped); nothing else of Apple's payload is kept.
    shown.userInfo = ["c": sealed]
    return shown
  }

  /// The generic line, whatever text Apple brought (the relay sets it, and
  /// the relay isn't trusted with words), and nothing a tap could act on.
  static func generic(_ content: UNNotificationContent) -> UNNotificationContent {
    let generic = UNMutableNotificationContent()
    generic.title = "Claktory"
    generic.body = "Claktory has something for you"
    generic.sound = content.sound
    generic.threadIdentifier = content.threadIdentifier
    generic.interruptionLevel = .active
    return generic
  }
}
