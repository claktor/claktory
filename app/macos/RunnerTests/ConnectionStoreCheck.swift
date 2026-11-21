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
  var writeFails = false

  func read() throws -> Data? {
    if fails { throw KeychainError(status: errSecInteractionNotAllowed) }
    return data
  }

  func write(_ data: Data) throws {
    if fails || writeFails { throw KeychainError(status: errSecAuthFailed) }
    self.data = data
  }

  func delete() throws {
    if fails { throw KeychainError(status: errSecAuthFailed) }
    data = nil
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

    func code(_ run: () throws -> Void) -> String? {
      do {
        try run()
        return nil
      } catch let failure as StoreFailure {
        return failure.code
      } catch {
        return "\(error)"
      }
    }

    // Ad hoc: user defaults, as before.
    do {
      let defaults = freshDefaults()
      let store = ConnectionStore(defaults: defaults, keychain: nil, log: log)
      check(try store.read().isEmpty, "nothing kept yet")
      try store.write(saved)
      check(same(defaults.dictionary(forKey: ConnectionStore.defaultsKey) ?? [:], saved), "ad hoc writes user defaults")
      check(same(try store.read(), saved), "ad hoc reads them back")
      try store.reset()
      check(try store.read().isEmpty, "started over")
    }

    // First launch with the Keychain: what user defaults had moves there.
    do {
      let defaults = freshDefaults()
      defaults.set(saved, forKey: ConnectionStore.defaultsKey)
      let keychain = FakeKeychain()
      let store = ConnectionStore(defaults: defaults, keychain: keychain, log: log)
      check(same(try store.read(), saved), "the old connections are read")
      check(defaults.object(forKey: ConnectionStore.defaultsKey) == nil, "and leave user defaults")
      check(same(ConnectionStore.decode(keychain.data ?? Data()) ?? [:], saved), "for the Keychain")
      check(logged.last?.contains("moved") == true, "the move is logged")

      // The next launch reads the Keychain; writes go there only.
      let again = ConnectionStore(defaults: defaults, keychain: keychain, log: log)
      check(same(try again.read(), saved), "read from the Keychain")
      try again.write(["current": ""])
      check(same(ConnectionStore.decode(keychain.data!) ?? [:], ["current": ""]), "written to the Keychain")
      check(defaults.object(forKey: ConnectionStore.defaultsKey) == nil, "user defaults stay empty")
    }

    // The Keychain can't be read: an error, never an empty store, and
    // nothing written over it until a read works or the person starts over.
    do {
      let defaults = freshDefaults()
      let keychain = FakeKeychain()
      keychain.data = try! ConnectionStore.encode(saved)
      keychain.fails = true
      let store = ConnectionStore(defaults: defaults, keychain: keychain, log: log)
      check(code { _ = try store.read() } == "credentials_unavailable", "a locked Keychain is an error")
      check(code { try store.write(["current": ""]) } == "credentials_read_failed", "a write is refused")
      check(defaults.object(forKey: ConnectionStore.defaultsKey) == nil, "nothing went to user defaults")
      keychain.fails = false
      check(same(ConnectionStore.decode(keychain.data!) ?? [:], saved), "the Keychain is as it was")
      // Try again: the read works, and so do writes.
      check(same(try store.read(), saved), "read once it can be")
      try store.write(["current": "later"])
      check(same(ConnectionStore.decode(keychain.data!) ?? [:], ["current": "later"]), "written after")
    }

    // What's there isn't connections: its own error; Start over clears it.
    do {
      let defaults = freshDefaults()
      let keychain = FakeKeychain()
      keychain.data = Data("not json".utf8)
      let store = ConnectionStore(defaults: defaults, keychain: keychain, log: log)
      check(code { _ = try store.read() } == "credentials_unreadable", "unreadable")
      check(code { try store.write(saved) } == "credentials_read_failed", "and not written over")
      try store.reset()
      check(keychain.data == nil, "start over removes it")
      check(try store.read().isEmpty, "a fresh store")
      try store.write(saved)
      check(same(try store.read(), saved), "written again")
      keychain.fails = true
      check(code { try store.reset() } == "credentials_reset_failed", "a reset the Keychain refuses says so")
    }

    // A Mac whose Keychain refuses a write: user defaults until the next
    // launch, which moves it back. The iPhone's fails instead.
    do {
      let defaults = freshDefaults()
      let keychain = FakeKeychain()
      keychain.data = try! ConnectionStore.encode(saved)
      let store = ConnectionStore(defaults: defaults, keychain: keychain, log: log)
      check(same(try store.read(), saved), "read")
      keychain.writeFails = true
      try store.write(["current": "later"])
      check(same(ConnectionStore.decode(keychain.data!) ?? [:], saved), "the Keychain isn't overwritten this run")
      check(same(defaults.dictionary(forKey: ConnectionStore.defaultsKey) ?? [:], ["current": "later"]), "user defaults meanwhile")
      keychain.writeFails = false
      let next = ConnectionStore(defaults: defaults, keychain: keychain, log: log)
      check(same(try next.read(), ["current": "later"]), "the newest wins at the next launch")
      check(same(ConnectionStore.decode(keychain.data!) ?? [:], ["current": "later"]), "and moves to the Keychain")
      check(defaults.object(forKey: ConnectionStore.defaultsKey) == nil, "leaving user defaults")

      let phone = ConnectionStore(defaults: freshDefaults(), keychain: keychain, fallsBack: false, log: log)
      check(same(try phone.read(), ["current": "later"]), "the phone reads")
      keychain.writeFails = true
      check(code { try phone.write(saved) } == "credentials_write_failed", "the phone's failed write says so")
    }

    // The real Keychain: an item of this check's own.
    do {
      let item = KeychainItem(
        service: "Claktory connections check", account: UUID().uuidString, label: "Claktory check")
      defer { try? item.delete() }
      check((try? item.read()) == .some(nil), "no item yet")
      try item.write(Data("one".utf8))
      check((try? item.read()) == Data("one".utf8), "written")
      try item.write(Data("two".utf8))
      check((try? item.read()) == Data("two".utf8), "updated")
      try item.delete()
      check((try? item.read()) == .some(nil), "removed")
      try item.delete()  // none there: no failure
    } catch {
      check(false, "keychain: \(error)")
    }

    for name in names { UserDefaults.standard.removePersistentDomain(forName: name) }
    print(failures == 0 ? "ConnectionStore: all checks passed" : "ConnectionStore: \(failures) failed")
    exit(failures == 0 ? 0 : 1)
  }
}
