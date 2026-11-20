import Foundation
import Security

/// Where this Mac keeps its connections (app/lib/connections.dart): the
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
  private var keychain: SecretStore?
  private let log: (String) -> Void

  init(defaults: UserDefaults, keychain: SecretStore?, log: @escaping (String) -> Void) {
    self.defaults = defaults
    self.keychain = keychain
    self.log = log
  }

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

  /// What's kept. Any left in user defaults (from before, or written while
  /// the Keychain failed) is newest: it moves to the Keychain and leaves
  /// user defaults.
  func read() -> [String: Any] {
    let kept = defaults.dictionary(forKey: Self.defaultsKey)
    guard let keychain else { return kept ?? [:] }
    do {
      if let kept {
        try keychain.write(Self.encode(kept))
        defaults.removeObject(forKey: Self.defaultsKey)
        log("moved from user defaults to the Keychain")
        return kept
      }
      guard let data = try keychain.read() else { return [:] }
      return Self.decode(data) ?? [:]
    } catch {
      fallBack(error)
      return kept ?? [:]
    }
  }

  func write(_ connections: [String: Any]) {
    if let keychain {
      do {
        try keychain.write(Self.encode(connections))
        return
      } catch {
        fallBack(error)
      }
    }
    defaults.set(connections, forKey: Self.defaultsKey)
  }

  /// The Keychain failed (denied, locked): user defaults until the app
  /// starts again, which then moves what was written there back.
  private func fallBack(_ error: Error) {
    keychain = nil
    log("the Keychain failed (\(error)), so they're kept in user defaults until Claktory starts again")
  }

  static func encode(_ connections: [String: Any]) throws -> Data {
    try JSONSerialization.data(withJSONObject: connections)
  }

  static func decode(_ data: Data) -> [String: Any]? {
    (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
  }

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
}

/// One secret blob, kept somewhere only this app reads.
protocol SecretStore {
  /// Nil when there's none yet; throws when it can't be read.
  func read() throws -> Data?
  func write(_ data: Data) throws
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
  /// What Keychain Access shows it as.
  let label: String

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
    let update = [kSecValueData as String: data]
    let status = SecItemUpdate(query as CFDictionary, update as CFDictionary)
    if status == errSecSuccess { return }
    guard status == errSecItemNotFound else { throw KeychainError(status: status) }
    var add = query
    add[kSecValueData as String] = data
    add[kSecAttrLabel as String] = label
    let added = SecItemAdd(add as CFDictionary, nil)
    guard added == errSecSuccess else { throw KeychainError(status: added) }
  }

  /// Removes it (tests).
  func delete() {
    SecItemDelete(query as CFDictionary)
  }
}
