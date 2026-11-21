import Foundation
import Security

/// Where this Mac keeps its connections (app/lib/connections.dart), and the
/// iPhone too (its Runner builds this same file): the
/// pairing codes, each holding a token that lets in whoever has it, this
/// Mac's iroh key for each, which one the window shows and what the person
/// calls them. They're in the Keychain, readable by this app only, like the
/// iPhone's (ios/Runner/AppDelegate.swift); user defaults are a plist any
/// process of the person's can read, so a copy of it could pose as this Mac.
///
/// The Keychain trusts the app by its code signature. Only a build signed
/// with a stable identity (Developer ID, Apple Development: a team) keeps
/// that trust across builds; an ad hoc one (Claktory Dev, a release built
/// without the Developer ID) would be asked for access by every new build,
/// so it keeps them in user defaults instead, and says so in the log.
final class ConnectionStore {
  /// The user defaults key they were kept under before (and still are, ad hoc).
  static let defaultsKey = "Connections"

  private let defaults: UserDefaults
  /// Nil when this build can't use the Keychain.
  private let keychain: SecretStore?
  /// Off once the Keychain failed to write on a Mac, which then writes user
  /// defaults until the app starts again.
  private var keychainWorks = true
  /// Whether a write the Keychain refuses goes to user defaults instead
  /// (the Mac) or fails (the iPhone, where nothing secret goes in a plist).
  private let fallsBack: Bool
  /// The last read failed: nothing is written until one succeeds or the
  /// person starts over, so a store that couldn't be read is never written
  /// over with nothing (Raj's DESIGN.md §5).
  private(set) var readFailed = false
  private let log: (String) -> Void

  init(
    defaults: UserDefaults, keychain: SecretStore?, fallsBack: Bool = true,
    log: @escaping (String) -> Void
  ) {
    self.defaults = defaults
    self.keychain = keychain
    self.fallsBack = fallsBack
    self.log = log
  }

  #if os(macOS)
    /// This app's: in the login keychain under its own bundle id, so
    /// Claktory and Claktory Dev never share or ask for each other's.
    static func forThisApp(displayName: String) -> ConnectionStore {
      let log = { (message: String) in NSLog("Claktory connections: %@", message) }
      guard signedByATeam() else {
        log("this build is signed ad hoc, so they stay in user defaults: the Keychain would ask for access after every build")
        return ConnectionStore(defaults: .standard, keychain: nil, log: log)
      }
      let item = KeychainItem(
        service: "Claktory connections",
        account: Bundle.main.bundleIdentifier ?? "dev.factory.app",
        label: "\(displayName) connections")
      return ConnectionStore(defaults: .standard, keychain: item, log: log)
    }
  #else
    /// The iPhone's: in its keychain, readable after the first unlock, on
    /// this device only.
    static func forThisApp() -> ConnectionStore {
      let item = KeychainItem(
        service: "Claktory connections", account: "connections", label: nil,
        accessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly)
      return ConnectionStore(
        defaults: .standard, keychain: item, fallsBack: false,
        log: { NSLog("Claktory connections: %@", $0) })
    }
  #endif

  /// What's kept: empty only when nothing ever was. Any left in user
  /// defaults (from before, or written while the Keychain failed) is
  /// newest: it moves to the Keychain and leaves user defaults. Throws a
  /// [StoreFailure] when it can't be read, saying why
  /// (credentials_unavailable: locked, denied, busy, worth trying again;
  /// credentials_unreadable: there but not what was written).
  func read() throws -> [String: Any] {
    let kept = defaults.dictionary(forKey: Self.defaultsKey)
    guard let keychain, keychainWorks else {
      readFailed = false
      return kept ?? [:]
    }
    if let kept {
      do {
        try keychain.write(Self.encode(kept))
        defaults.removeObject(forKey: Self.defaultsKey)
        log("moved from user defaults to the Keychain")
      } catch {
        fallBack(error)
      }
      readFailed = false
      return kept
    }
    let data: Data?
    do {
      data = try keychain.read()
    } catch {
      readFailed = true
      log("couldn't read them: \(error)")
      throw StoreFailure(code: "credentials_unavailable", message: "\(error)")
    }
    guard let data else {
      readFailed = false
      return [:]
    }
    guard let connections = Self.decode(data) else {
      readFailed = true
      log("what's kept can't be read as connections")
      throw StoreFailure(
        code: "credentials_unreadable", message: "What's kept isn't readable as connections.")
    }
    readFailed = false
    return connections
  }

  /// Keeps [connections]; refused (credentials_read_failed) while the last
  /// read failed, and credentials_write_failed when it can't be kept.
  func write(_ connections: [String: Any]) throws {
    if readFailed {
      throw StoreFailure(
        code: "credentials_read_failed",
        message: "They couldn't be read, so they aren't written over.")
    }
    if let keychain, keychainWorks {
      do {
        try keychain.write(Self.encode(connections))
        return
      } catch {
        guard fallsBack else {
          log("couldn't write them: \(error)")
          throw StoreFailure(code: "credentials_write_failed", message: "\(error)")
        }
        fallBack(error)
      }
    }
    defaults.set(connections, forKey: Self.defaultsKey)
  }

  /// Starts over, at the person's word: whatever's kept goes, wherever it
  /// is, and writing is allowed again. credentials_reset_failed when the
  /// Keychain won't let it go.
  func reset() throws {
    defaults.removeObject(forKey: Self.defaultsKey)
    if let keychain {
      do {
        try keychain.delete()
      } catch {
        log("couldn't remove them: \(error)")
        throw StoreFailure(code: "credentials_reset_failed", message: "\(error)")
      }
    }
    keychainWorks = true
    readFailed = false
    log("started over")
  }

  /// The Keychain failed to write (denied, locked): user defaults until the
  /// app starts again, which then moves what was written there back.
  private func fallBack(_ error: Error) {
    keychainWorks = false
    log("the Keychain failed (\(error)), so they're kept in user defaults until Claktory starts again")
  }

  static func encode(_ connections: [String: Any]) throws -> Data {
    try JSONSerialization.data(withJSONObject: connections)
  }

  static func decode(_ data: Data) -> [String: Any]? {
    (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
  }

  #if os(macOS)
    /// Whether this app's signature names a team: a Developer ID or Apple
    /// Development identity, which stays the same from build to build.
    static func signedByATeam() -> Bool {
      var running: SecCode?
      var code: SecStaticCode?
      var info: CFDictionary?
      guard SecCodeCopySelf([], &running) == errSecSuccess, let running,
        SecCodeCopyStaticCode(running, [], &code) == errSecSuccess, let code,
        SecCodeCopySigningInformation(
          code, SecCSFlags(rawValue: kSecCSSigningInformation), &info) == errSecSuccess,
        let info = info as? [String: Any]
      else { return false }
      let team = info[kSecCodeInfoTeamIdentifier as String] as? String
      return !(team ?? "").isEmpty
    }
  #endif
}

/// Why the connections couldn't be read, written or removed: [code] is the
/// app's (and Android's) name for it, [message] what the system said.
struct StoreFailure: Error, CustomStringConvertible {
  let code: String
  let message: String

  var description: String { "\(code): \(message)" }
}

/// One secret blob, kept somewhere only this app reads.
protocol SecretStore {
  /// Nil when there's none yet; throws when it can't be read.
  func read() throws -> Data?
  func write(_ data: Data) throws
  /// Removes it; none there is no failure.
  func delete() throws
}

struct KeychainError: Error, CustomStringConvertible {
  let status: OSStatus

  var description: String {
    let said = SecCopyErrorMessageString(status, nil) as String? ?? "unknown"
    return "\(said) (\(status))"
  }
}

/// A generic password in the login keychain. Its access list holds only
/// the app that made it, by its signature, so other apps are refused or
/// must be allowed by the person; and the login keychain never syncs to
/// iCloud.
struct KeychainItem: SecretStore {
  let service: String
  let account: String
  /// What Keychain Access shows it as (the Mac's).
  let label: String?
  /// When it can be read (the iPhone's: after the first unlock, this
  /// device only); the keychain's default when nil.
  var accessible: CFString? = nil

  private var query: [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: account,
    ]
  }

  func read() throws -> Data? {
    var lookup = query
    lookup[kSecReturnData as String] = true
    lookup[kSecMatchLimit as String] = kSecMatchLimitOne
    var item: CFTypeRef?
    let status = SecItemCopyMatching(lookup as CFDictionary, &item)
    if status == errSecItemNotFound { return nil }
    guard status == errSecSuccess, let data = item as? Data else {
      throw KeychainError(status: status)
    }
    return data
  }

  func write(_ data: Data) throws {
    var update: [String: Any] = [kSecValueData as String: data]
    if let accessible { update[kSecAttrAccessible as String] = accessible }
    let status = SecItemUpdate(query as CFDictionary, update as CFDictionary)
    if status == errSecSuccess { return }
    guard status == errSecItemNotFound else { throw KeychainError(status: status) }
    var add = query.merging(update) { $1 }
    if let label { add[kSecAttrLabel as String] = label }
    let added = SecItemAdd(add as CFDictionary, nil)
    guard added == errSecSuccess else { throw KeychainError(status: added) }
  }

  func delete() throws {
    let status = SecItemDelete(query as CFDictionary)
    guard status == errSecSuccess || status == errSecItemNotFound else {
      throw KeychainError(status: status)
    }
  }
}
