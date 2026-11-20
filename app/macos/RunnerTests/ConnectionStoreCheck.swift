// Checks ConnectionStore (Runner/ConnectionStore.swift) without the app, so
// nothing opens a window or touches the app's own defaults or Keychain item:
//
//   cd app/macos && swiftc -o /tmp/connection-store-check \
//     Runner/ConnectionStore.swift RunnerTests/ConnectionStoreCheck.swift \
//     && /tmp/connection-store-check
//
// It uses a throwaway user defaults suite, a Keychain stand-in, and one real
// login-keychain item of its own that it removes again.
import Foundation

final class FakeKeychain: SecretStore {
  var data: Data?
  var fails = false

  func read() throws -> Data? {
    if fails { throw KeychainError(status: errSecAuthFailed) }
    return data
  }

  func write(_ data: Data) throws {
    if fails { throw KeychainError(status: errSecAuthFailed) }
    self.data = data
  }
}

@main
enum ConnectionStoreCheck {
  static func main() throws {
    var names: [String] = []
    var failures = 0
    func check(_ ok: Bool, _ what: String, line: Int = #line) {
      if !ok {
        failures += 1
        print("FAILED (line \(line)): \(what)")
      }
    }

    func freshDefaults() -> UserDefaults {
      let name = "claktory-connection-store-check-\(UUID().uuidString)"
      let defaults = UserDefaults(suiteName: name)!
      defaults.removePersistentDomain(forName: name)
      names.append(name)
      return defaults
    }

    let saved: [String: Any] = [
      "current": "factory://?t=secret&e=abc",
      "saved": ["factory://?t=secret&e=abc"],
      "keys": ["factory://?t=secret&e=abc": String(repeating: "a", count: 64)],
      "names": ["": "Studio"],
    ]
    func same(_ a: [String: Any], _ b: [String: Any]) -> Bool {
      NSDictionary(dictionary: a).isEqual(to: b)
    }
    var logged: [String] = []
    let log = { (message: String) in logged.append(message) }

    // Ad hoc: user defaults, as before.
    do {
      let defaults = freshDefaults()
      let store = ConnectionStore(defaults: defaults, keychain: nil, log: log)
      check(store.read().isEmpty, "nothing kept yet")
      store.write(saved)
      check(same(defaults.dictionary(forKey: ConnectionStore.defaultsKey) ?? [:], saved), "ad hoc writes user defaults")
      check(same(store.read(), saved), "ad hoc reads them back")
    }

    // First launch with the Keychain: what user defaults had moves there.
    do {
      let defaults = freshDefaults()
      defaults.set(saved, forKey: ConnectionStore.defaultsKey)
      let keychain = FakeKeychain()
      let store = ConnectionStore(defaults: defaults, keychain: keychain, log: log)
      check(same(store.read(), saved), "the old connections are read")
      check(defaults.object(forKey: ConnectionStore.defaultsKey) == nil, "and leave user defaults")
      check(same(ConnectionStore.decode(keychain.data ?? Data()) ?? [:], saved), "for the Keychain")
      check(logged.last?.contains("moved") == true, "the move is logged")

      // The next launch reads the Keychain; writes go there only.
      let again = ConnectionStore(defaults: defaults, keychain: keychain, log: log)
      check(same(again.read(), saved), "read from the Keychain")
      again.write(["current": ""])
      check(same(ConnectionStore.decode(keychain.data!) ?? [:], ["current": ""]), "written to the Keychain")
      check(defaults.object(forKey: ConnectionStore.defaultsKey) == nil, "user defaults stay empty")
    }

    // The Keychain fails: user defaults for now, never overwriting the Keychain,
    // and back to the Keychain once it works.
    do {
      let defaults = freshDefaults()
      let keychain = FakeKeychain()
      keychain.data = try! ConnectionStore.encode(saved)
      keychain.fails = true
      let store = ConnectionStore(defaults: defaults, keychain: keychain, log: log)
      check(store.read().isEmpty, "nothing readable")
      check(logged.last?.contains("failed") == true, "the failure is logged")
      keychain.fails = false
      store.write(["current": "later"])
      check(same(ConnectionStore.decode(keychain.data!) ?? [:], saved), "the Keychain isn't overwritten this run")
      check(same(defaults.dictionary(forKey: ConnectionStore.defaultsKey) ?? [:], ["current": "later"]), "user defaults meanwhile")
      let next = ConnectionStore(defaults: defaults, keychain: keychain, log: log)
      check(same(next.read(), ["current": "later"]), "the newest wins at the next launch")
      check(same(ConnectionStore.decode(keychain.data!) ?? [:], ["current": "later"]), "and moves to the Keychain")
      check(defaults.object(forKey: ConnectionStore.defaultsKey) == nil, "leaving user defaults")
    }

    // The real Keychain: an item of this check's own.
    do {
      let item = KeychainItem(
        service: "Claktory connections check", account: UUID().uuidString, label: "Claktory check")
      defer { item.delete() }
      check((try? item.read()) == .some(nil), "no item yet")
      try item.write(Data("one".utf8))
      check((try? item.read()) == Data("one".utf8), "written")
      try item.write(Data("two".utf8))
      check((try? item.read()) == Data("two".utf8), "updated")
      item.delete()
      check((try? item.read()) == .some(nil), "removed")
    } catch {
      check(false, "keychain: \(error)")
    }

    for name in names { UserDefaults.standard.removePersistentDomain(forName: name) }
    print(failures == 0 ? "ConnectionStore: all checks passed" : "ConnectionStore: \(failures) failed")
    exit(failures == 0 ? 0 : 1)
  }
}
