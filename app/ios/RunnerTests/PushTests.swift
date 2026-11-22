import CryptoKit
import Foundation
import Testing
import UserNotifications

@testable import Runner

/// Opening pushes (Push.swift), against the backend's own test vectors
/// (crates/factoryd/src/notify/push-vectors.json, made by notify/push.rs),
/// and Fugo's rules: only what opens, new and fresh shows; a tap opens what
/// it's about only for a push that showed.
struct PushTests {
  struct Vectors: Decodable {
    let key_hex: String
    let vectors: [Vector]
  }

  struct Vector: Decodable {
    let name: String
    let expect: String
    let ctr: UInt64
    let c: String
    let plaintext: Plain?
  }

  struct Plain: Decodable {
    let sent_at: Double
    let ttl: Double
    let claktory: String
    let title: String
    let body: String
    let badge: Int
  }

  static func vectors() throws -> Vectors {
    let file = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
      .deletingLastPathComponent()
      .appendingPathComponent("crates/factoryd/src/notify/push-vectors.json")
    return try JSONDecoder().decode(Vectors.self, from: Data(contentsOf: file))
  }

  static func key(_ hex: String) -> Data {
    var bytes = [UInt8]()
    var index = hex.startIndex
    while index < hex.endIndex {
      let next = hex.index(index, offsetBy: 2)
      bytes.append(UInt8(hex[index..<next], radix: 16)!)
      index = next
    }
    return Data(bytes)
  }

  /// A push as Apple brings it: the relay's line, and the sealed text.
  static func push(_ sealed: String?, title: String = "Claktory") -> UNNotificationContent {
    let content = UNMutableNotificationContent()
    content.title = title
    content.body = "Claktory has something for you"
    content.threadIdentifier = "Bv6H5l7hdJgG_h268SXQmg"
    if let sealed { content.userInfo = ["c": sealed, "aps": ["mutable-content": 1]] }
    return content
  }

  /// Seals `json` as a Claktory does, padded to 512 bytes.
  static func seal(_ json: [String: Any], key: Data) throws -> String {
    var plain = try JSONSerialization.data(withJSONObject: json)
    plain.append(contentsOf: [UInt8](repeating: 0x20, count: 512 - plain.count))
    let box = try AES.GCM.seal(plain, using: SymmetricKey(data: key))
    return try #require(box.combined).base64EncodedString()
  }

  static func notice(ctr: UInt64, sentAt: Double, ttl: Double = 86400) -> [String: Any] {
    [
      "v": 1, "ctr": ctr, "sent_at": sentAt, "ttl": ttl, "claktory": "Studio",
      "kind": "alert", "title": "Pericolo", "body": "account.claktory.com isn't answering",
      "badge": 1, "urgent": true,
      "target": ["agent": "devops", "pipe": "", "chat": "", "seq": 7],
    ]
  }

  @Test func opensTheBackendsVectorsInOrder() throws {
    let vectors = try Self.vectors()
    let keys = MemoryPushKeys()
    keys.keys["e1"] = PushKey(key: Self.key(vectors.key_hex))
    // Checked at the time they were sealed, as their file says.
    let now = Date(timeIntervalSince1970: 1_800_000_000)
    #expect(vectors.vectors.count == 7)
    for vector in vectors.vectors {
      let shown = PushOpener.content(for: Self.push(vector.c), keys: keys, now: now)
      if vector.expect == "shows" {
        let plain = try #require(vector.plaintext, "\(vector.name)")
        #expect(shown.title == plain.title, "\(vector.name)")
        #expect(shown.subtitle == plain.claktory, "\(vector.name)")
        #expect(shown.body == plain.body, "\(vector.name)")
        #expect(shown.badge == NSNumber(value: plain.badge), "\(vector.name)")
        #expect(keys.keys["e1"]?.ctr == vector.ctr, "\(vector.name)")
        // A tap opens what it's about, in that Claktory.
        let target = try #require(
          PushOpener.tapped(shown.userInfo["c"], keys: keys.all()), "\(vector.name)")
        #expect(target["claktory"] as? String == "e1")
        #expect(target["agentId"] as? String == "devops")
        #expect(target["seq"] as? Int == 42)
      } else {
        #expect(shown.title == "Claktory", "\(vector.name)")
        #expect(shown.body == "Claktory has something for you", "\(vector.name)")
        #expect(shown.subtitle == "", "\(vector.name)")
        #expect(shown.userInfo.isEmpty, "\(vector.name)")
        #expect(shown.badge == nil, "\(vector.name)")
      }
    }
    // The replay showed the generic line, and its tap opens nothing:
    // only the pushes shown are kept.
    let replay = try #require(vectors.vectors.last)
    #expect(PushOpener.tapped(replay.c, keys: keys.all()) == nil)
    #expect(keys.keys["e1"]?.shown.count == 4)
  }

  @Test func aPushNoKeyOpensShowsTheGenericLineAndItsTapOnlyOpensTheApp() throws {
    let keys = MemoryPushKeys()
    keys.keys["e1"] = PushKey(key: Data(repeating: 7, count: 32))
    let sealed = try Self.seal(
      Self.notice(ctr: 1, sentAt: 1_800_000_000_000), key: Data(repeating: 9, count: 32))
    let now = Date(timeIntervalSince1970: 1_800_000_000)
    // The relay's own words don't show either.
    let shown = PushOpener.content(
      for: Self.push(sealed, title: "Apple ID: verify now"), keys: keys, now: now)
    #expect(shown.title == "Claktory")
    #expect(shown.body == "Claktory has something for you")
    #expect(shown.userInfo.isEmpty)
    #expect(PushOpener.tapped(sealed, keys: keys.all()) == nil)
    #expect(keys.keys["e1"]?.ctr == 0)
    // Nothing sealed at all.
    #expect(PushOpener.content(for: Self.push(nil), keys: keys, now: now).title == "Claktory")
    #expect(PushOpener.open("not base64!", keys: keys.all(), now: now) == .generic(.unopened))
  }

  @Test func aPushPastItsTtlShowsTheGenericLine() throws {
    let key = Data(repeating: 7, count: 32)
    let keys = MemoryPushKeys()
    keys.keys["e1"] = PushKey(key: key)
    let sentAt = 1_800_000_000.0
    let sealed = try Self.seal(Self.notice(ctr: 1, sentAt: sentAt * 1000, ttl: 3600), key: key)
    // Within the ttl and a few minutes past it: shows.
    let late = Date(timeIntervalSince1970: sentAt + 3600 + PushOpener.slack - 1)
    guard case .shows = PushOpener.open(sealed, keys: keys.all(), now: late) else {
      Issue.record("a push still fresh didn't show")
      return
    }
    // Past that: too old.
    let stale = Date(timeIntervalSince1970: sentAt + 3600 + PushOpener.slack + 1)
    #expect(PushOpener.open(sealed, keys: keys.all(), now: stale) == .generic(.stale))
    let shown = PushOpener.content(for: Self.push(sealed), keys: keys, now: stale)
    #expect(shown.body == "Claktory has something for you")
    #expect(PushOpener.tapped(sealed, keys: keys.all()) == nil)
  }

  @Test func eachClaktorysKeyOpensItsOwnAndSaysWhichItWas() throws {
    let studio = Data(repeating: 1, count: 32)
    let home = Data(repeating: 2, count: 32)
    let keys = MemoryPushKeys()
    keys.keys["studio"] = PushKey(key: studio)
    keys.keys["home"] = PushKey(key: home, ctr: 40)
    let now = Date(timeIntervalSince1970: 1_800_000_000)
    let sealed = try Self.seal(Self.notice(ctr: 41, sentAt: 1_800_000_000_000), key: home)
    let shown = PushOpener.content(for: Self.push(sealed), keys: keys, now: now)
    #expect(shown.title == "Pericolo")
    #expect(shown.subtitle == "Studio")
    #expect(keys.keys["home"]?.ctr == 41)
    #expect(keys.keys["studio"]?.ctr == 0)
    #expect(PushOpener.tapped(sealed, keys: keys.all())?["claktory"] as? String == "home")
  }

  @Test func aPushThatCantBeKeptShowsTheGenericLine() throws {
    final class Full: PushKeyStore {
      var keys: [String: PushKey]
      init(_ keys: [String: PushKey]) { self.keys = keys }
      func all() -> [String: PushKey] { keys }
      func save(_ key: PushKey, for claktory: String) -> Bool { false }
      func remove(_ claktory: String) {}
      var lastStatus: OSStatus { errSecIO }
    }
    let key = Data(repeating: 7, count: 32)
    let sealed = try Self.seal(Self.notice(ctr: 1, sentAt: 1_800_000_000_000), key: key)
    let shown = PushOpener.content(
      for: Self.push(sealed), keys: Full(["e1": PushKey(key: key)]),
      now: Date(timeIntervalSince1970: 1_800_000_000))
    #expect(shown.body == "Claktory has something for you")
  }

  /// v0.1.16's TestFlight build was archived unsigned, so Info.plist's
  /// group lost its team prefix and the keychain refused every key: the
  /// prefix is found again at run time.
  @Test func aGroupTheBuildLeftWithoutItsTeamGetsItBack() {
    let bare = "com.claktory.app.notifications"
    let team = { "ABCDE12345.com.claktory.app" as String? }
    #expect(KeychainPushKeys.entitled(bare, defaultGroup: team) == "ABCDE12345.\(bare)")
    #expect(
      KeychainPushKeys.entitled("ZYXWV98765.\(bare)", defaultGroup: team) == "ZYXWV98765.\(bare)")
    // None to be had: left as it is, so saving fails and says why.
    #expect(KeychainPushKeys.entitled(bare, defaultGroup: { nil }) == bare)
    #expect(KeychainPushKeys.entitled(bare, defaultGroup: { "com.claktory.app" }) == bare)
    // Not filled in at all, or none: the app's own group.
    #expect(KeychainPushKeys.entitled("$(AppIdentifierPrefix)\(bare)", defaultGroup: team) == nil)
    #expect(KeychainPushKeys.entitled(nil, defaultGroup: team) == nil)
    #expect(KeychainPushKeys.teamPrefix(of: "ABCDE12345.com.x") == "ABCDE12345")
    #expect(KeychainPushKeys.teamPrefix(of: "com.claktory.app") == nil)
    #expect(KeychainPushKeys.teamPrefix(of: "ABCDE12345") == nil)
  }

  /// The keychain itself keeps a key in the group this build names, and in
  /// that group with its team left out, as an unsigned archive leaves it.
  @Test func theKeychainKeepsAKeyEvenWhenTheBuildLeftOutTheTeam() throws {
    let named = try #require(
      Bundle.main.object(forInfoDictionaryKey: "ClaktoryPushKeychainGroup") as? String)
    let team = try #require(KeychainPushKeys.appDefaultGroup().flatMap(KeychainPushKeys.teamPrefix))
    let bare = named.hasPrefix("\(team).") ? String(named.dropFirst(team.count + 1)) : named
    for group in [named, bare] {
      let keys = KeychainPushKeys(group: group)
      #expect(keys.group == "\(team).\(bare)")
      let account = "test-\(UUID().uuidString)"
      let key = PushKey(key: Data(repeating: 9, count: 32))
      #expect(keys.save(key, for: account), "status \(keys.lastStatus)")
      #expect(keys.all()[account]?.key == key.key)
      keys.remove(account)
      #expect(keys.all()[account] == nil)
    }
  }

  @Test func onlyTheNewestShownCountersAreKept() {
    var key = PushKey(key: Data(repeating: 7, count: 32))
    for ctr in 1...UInt64(PushKey.kept + 5) { key.show(ctr, sealed: "push \(ctr)") }
    #expect(key.shown.count == PushKey.kept)
    #expect(key.shown.first == PushKey.digest("push 6"))
    #expect(key.ctr == UInt64(PushKey.kept + 5))
  }

  /// The keys in the keychain group the app shares with its extension, as
  /// the app's Info.plist names it.
  @Test func theKeychainKeepsAKeyInTheSharedGroup() throws {
    let store = KeychainPushKeys()
    let group = try #require(store.group)
    #expect(group.hasSuffix(".com.claktory.app.notifications"))
    let id = "test-\(UUID().uuidString)"
    defer { store.remove(id) }
    var key = PushKey(key: Data(repeating: 3, count: 32))
    #expect(store.save(key, for: id))
    key.show(9, sealed: "push")
    #expect(store.save(key, for: id))
    #expect(store.all()[id] == key)
    store.remove(id)
    #expect(store.all()[id] == nil)
  }

  /// Only the sealed notice makes a push urgent (time-sensitive, through
  /// Focus): never Apple's payload, which the relay writes.
  @Test func onlyTheSealedNoticeMakesAPushUrgent() throws {
    let key = Data(repeating: 7, count: 32)
    let keys = MemoryPushKeys()
    keys.keys["e1"] = PushKey(key: key)
    let now = Date(timeIntervalSince1970: 1_800_000_000)
    func push(_ sealed: String) -> UNNotificationContent {
      let content = Self.push(sealed).mutableCopy() as! UNMutableNotificationContent
      content.interruptionLevel = .timeSensitive
      return content
    }
    let unopened = try Self.seal(
      Self.notice(ctr: 1, sentAt: 1_800_000_000_000), key: Data(repeating: 9, count: 32))
    #expect(PushOpener.content(for: push(unopened), keys: keys, now: now).interruptionLevel == .active)
    var calm = Self.notice(ctr: 2, sentAt: 1_800_000_000_000)
    calm["urgent"] = false
    let shownCalm = PushOpener.content(for: push(try Self.seal(calm, key: key)), keys: keys, now: now)
    #expect(shownCalm.title == "Pericolo")
    #expect(shownCalm.interruptionLevel == .active)
    let urgent = Self.notice(ctr: 3, sentAt: 1_800_000_000_000)
    let shownUrgent = PushOpener.content(
      for: Self.push(try Self.seal(urgent, key: key)), keys: keys, now: now)
    #expect(shownUrgent.interruptionLevel == .timeSensitive)
  }

  /// A tap: what the app posted says where; a push, only what it opens to.
  @Test func aTapOnAPushTrustsOnlyWhatItOpensTo() throws {
    let keys = MemoryPushKeys()
    let local = UNMutableNotificationContent()
    local.userInfo = ["target": ["claktory": "e1", "agentId": "devops"]]
    let posted = UNNotificationRequest(identifier: "n1", content: local, trigger: nil)
    #expect(Notifications.target(of: posted, keys: keys)?["agentId"] as? String == "devops")
  }
}
