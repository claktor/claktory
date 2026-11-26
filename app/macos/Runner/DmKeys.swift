// The keys of end-to-end direct messages (dms-e2ee.md §5.4, E6): the
// iPhone's, iPad's and Mac's half of `factory/dm-keys`. The contract is
// app/lib/people/dm_keys.dart's; Android's half is DmKeys.kt. This only keeps
// opaque bytes under a name, gives them back and deletes them: the app's Rust
// makes every key and does all signing and MLS.
//
// Kept in the keychain as generic passwords of their own service, this
// device only (never synchronized: no iCloud Keychain), readable after the
// first unlock (as the local store's file is), with no prompt:
// - iPhone and iPad: the app's own keychain group, the first of its
//   keychain-access-groups, never the notifications group the extension
//   shares (the extension never reads direct messages).
// - Mac: the data-protection keychain, in the group the signed entitlements
//   give (TEAM.dev.factory.app, from the release's provisioning profile),
//   never the login keychain or user defaults. Without them (Claktory Dev,
//   signed ad hoc) nothing can be kept, and `available` says so.
#if os(iOS)
  import UIKit
#else
  import AppKit
#endif
#if canImport(Flutter)
  import Flutter
#elseif canImport(FlutterMacOS)
  import FlutterMacOS
#endif
import Foundation
import Security

/// Why a call failed: the channel's error codes.
enum DmKeyFailure: Error, Equatable {
  /// Something is kept under the name but can't be read: never "absent".
  case unreadable(String)
  /// Anything else (a bad name, too much, the keychain refusing a write).
  case failed(String)

  var code: String {
    switch self {
    case .unreadable: return "unreadable"
    case .failed: return "failed"
    }
  }

  var message: String {
    switch self {
    case .unreadable(let message), .failed(let message): return message
    }
  }
}

final class DmKeyStore {
  static let service = "com.claktory.dm-keys"

  /// The most a value may be, as Android's half.
  static let valueLimit = 4096

  /// The name `available` writes and reads back: outside the names callers
  /// may use, so it never meets a key.
  static let probeName = "~available"

  /// The keychain group, where it must be named (the Mac's); nil: the app's
  /// own, the keychain's default (iPhone and iPad).
  let group: String?

  /// The Mac's data-protection keychain, rather than the login keychain.
  let dataProtection: Bool
  let service: String

  init(group: String?, dataProtection: Bool, service: String = DmKeyStore.service) {
    self.group = group
    self.dataProtection = dataProtection
    self.service = service
  }

  /// This app's.
  static func forThisApp() -> DmKeyStore {
    #if os(iOS)
      return DmKeyStore(group: nil, dataProtection: false)
    #else
      return DmKeyStore(group: entitledGroup(), dataProtection: true)
    #endif
  }

  #if os(macOS)
    /// The first keychain group the app's signature entitles it to; nil when
    /// it has none (signed ad hoc, or by a tool).
    static func entitledGroup() -> String? {
      guard let task = SecTaskCreateFromSelf(nil),
        let groups = SecTaskCopyValueForEntitlement(
          task, "keychain-access-groups" as CFString, nil) as? [String]
      else { return nil }
      return groups.first { !$0.isEmpty && !$0.contains("$(") }
    }
  #endif

  static func validName(_ name: String) -> Bool {
    (1...32).contains(name.utf8.count)
      && name.utf8.allSatisfy { byte in
        (UInt8(ascii: "a")...UInt8(ascii: "z")).contains(byte)
          || (UInt8(ascii: "0")...UInt8(ascii: "9")).contains(byte)
          || byte == UInt8(ascii: "_") || byte == UInt8(ascii: "-")
      }
  }

  private func query(_ name: String) -> [String: Any] {
    var query: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: name,
      kSecAttrSynchronizable as String: false,
    ]
    if let group { query[kSecAttrAccessGroup as String] = group }
    if dataProtection { query[kSecUseDataProtectionKeychain as String] = true }
    return query
  }

  /// The Mac needs its group: without one, nothing may be kept.
  private func ready() throws {
    #if os(macOS)
      guard group != nil else {
        throw DmKeyFailure.failed("This Mac app isn't entitled to a keychain group.")
      }
    #endif
  }

  func put(_ name: String, _ bytes: Data) throws {
    guard Self.validName(name) || name == Self.probeName else {
      throw DmKeyFailure.failed("Not a key's name.")
    }
    guard bytes.count <= Self.valueLimit else {
      throw DmKeyFailure.failed("More than \(Self.valueLimit) bytes.")
    }
    try ready()
    let update: [String: Any] = [
      kSecValueData as String: bytes,
      kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
    ]
    var status = SecItemUpdate(query(name) as CFDictionary, update as CFDictionary)
    if status == errSecItemNotFound {
      var add = query(name)
      add.merge(update) { $1 }
      status = SecItemAdd(add as CFDictionary, nil)
    }
    guard status == errSecSuccess else {
      throw DmKeyFailure.failed("The keychain didn't keep it (\(status)).")
    }
  }

  func get(_ name: String) throws -> Data? {
    guard Self.validName(name) || name == Self.probeName else {
      throw DmKeyFailure.failed("Not a key's name.")
    }
    try ready()
    var lookup = query(name)
    lookup[kSecReturnData as String] = true
    lookup[kSecMatchLimit as String] = kSecMatchLimitOne
    var found: CFTypeRef?
    let status = SecItemCopyMatching(lookup as CFDictionary, &found)
    return try Self.outcome(status, found)
  }

  /// What a read's [status] means: the bytes, nothing kept (nil), or why
  /// not. Something there that can't be read is `unreadable`, never nil;
  /// the keychain locked before the first unlock is `failed`, since what's
  /// kept is then unknown.
  static func outcome(_ status: OSStatus, _ found: CFTypeRef?) throws -> Data? {
    switch status {
    case errSecSuccess:
      guard let data = found as? Data else {
        throw DmKeyFailure.unreadable("The keychain gave back no bytes.")
      }
      return data
    case errSecItemNotFound:
      return nil
    case errSecInteractionNotAllowed:
      throw DmKeyFailure.failed("The keychain is locked until the first unlock.")
    default:
      throw DmKeyFailure.unreadable("The keychain couldn't read it (\(status)).")
    }
  }

  func delete(_ name: String) throws {
    guard Self.validName(name) || name == Self.probeName else {
      throw DmKeyFailure.failed("Not a key's name.")
    }
    try ready()
    let status = SecItemDelete(query(name) as CFDictionary)
    guard status == errSecSuccess || status == errSecItemNotFound else {
      throw DmKeyFailure.failed("The keychain didn't delete it (\(status)).")
    }
  }

  /// Whether keys can be kept here: a write, a read back and a delete just
  /// worked, in the very keychain and group the keys go to.
  func available() -> Bool {
    let probe = Data((0..<32).map { _ in UInt8.random(in: 0...255) })
    do {
      try put(Self.probeName, probe)
      defer { try? delete(Self.probeName) }
      return try get(Self.probeName) == probe
    } catch {
      return false
    }
  }

  /// The local store's folder, made if missing: Application Support/dm on
  /// iPhone and iPad (excluded from backup, protected until the first
  /// unlock), the app's own Application Support/<bundle id>/dm on a Mac
  /// (Claktory and Claktory Dev apart).
  static func folder() throws -> String {
    let files = FileManager.default
    guard let support = files.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
    else { throw DmKeyFailure.failed("No Application Support folder.") }
    #if os(iOS)
      var folder = support.appendingPathComponent("dm", isDirectory: true)
      var attributes: [FileAttributeKey: Any] = [
        .protectionKey: FileProtectionType.completeUntilFirstUserAuthentication
      ]
    #else
      guard let bundle = Bundle.main.bundleIdentifier, !bundle.isEmpty else {
        throw DmKeyFailure.failed("This app has no bundle identifier.")
      }
      var folder = support.appendingPathComponent(bundle, isDirectory: true)
        .appendingPathComponent("dm", isDirectory: true)
      var attributes: [FileAttributeKey: Any] = [:]
    #endif
    attributes[.posixPermissions] = 0o700
    do {
      try files.createDirectory(at: folder, withIntermediateDirectories: true, attributes: attributes)
      var values = URLResourceValues()
      values.isExcludedFromBackup = true
      try folder.setResourceValues(values)
    } catch {
      throw DmKeyFailure.failed("The folder couldn't be made: \(error.localizedDescription)")
    }
    return folder.path
  }
}

enum DmKeys {
  static let channelName = "factory/dm-keys"

  /// The keychain off the main thread, one call at a time.
  private static let queue = DispatchQueue(label: "com.claktory.dm-keys")

  #if canImport(Flutter) || canImport(FlutterMacOS)
    /// [name]: another channel over another [store], such as the Mac's
    /// account key's (#529, `factory/account-key`).
    static func register(
      with messenger: FlutterBinaryMessenger, store: DmKeyStore = .forThisApp(),
      channel name: String = channelName
    ) {
      let channel = FlutterMethodChannel(name: name, binaryMessenger: messenger)
      channel.setMethodCallHandler { call, result in
        let args = call.arguments as? [String: Any] ?? [:]
        let name = args["name"] as? String ?? ""
        let bytes = (args["bytes"] as? FlutterStandardTypedData)?.data
        queue.async {
          let answer: Any?
          do {
            switch call.method {
            case "available":
              answer = store.available()
            case "put":
              guard let bytes else { throw DmKeyFailure.failed("No bytes.") }
              try store.put(name, bytes)
              answer = nil
            case "get":
              answer = try store.get(name).map { FlutterStandardTypedData(bytes: $0) }
            case "delete":
              try store.delete(name)
              answer = nil
            case "folder":
              answer = try DmKeyStore.folder()
            default:
              answer = FlutterMethodNotImplemented
            }
          } catch let failure as DmKeyFailure {
            answer = FlutterError(code: failure.code, message: failure.message, details: nil)
          } catch {
            answer = FlutterError(code: "failed", message: "\(error)", details: nil)
          }
          DispatchQueue.main.async { result(answer) }
        }
      }
    }
  #endif
}
