// Checks DmKeys.swift (factory/dm-keys, E6) without the app:
//
//   cd app/macos && swiftc -parse-as-library -o /tmp/dm-keys-check \
//     Runner/DmKeys.swift RunnerTests/DmKeysCheck.swift && /tmp/dm-keys-check
//
// Built so, it's signed ad hoc with no entitlements, as Claktory Dev is: no
// keychain group, so nothing may be kept, and nothing lands anywhere else
// (hostile test 12: not the login keychain, not user defaults). It never
// writes outside its own service, and makes no folder.
import Darwin
import Foundation
import Security

@main
enum DmKeysCheck {
  static func main() {
    var failures = 0
    func check(_ ok: Bool, _ what: String, line: Int = #line) {
      if !ok {
        failures += 1
        print("FAILED (line \(line)): \(what)")
      }
    }
    func failure(_ work: () throws -> Void) -> DmKeyFailure? {
      do {
        try work()
        return nil
      } catch let failure as DmKeyFailure {
        return failure
      } catch {
        return .failed("\(error)")
      }
    }

    // Names and sizes, as Android's half.
    check(DmKeyStore.validName("identity"), "identity is a name")
    check(DmKeyStore.validName("leaf_2-b"), "a-z 0-9 _ -")
    check(!DmKeyStore.validName(""), "empty")
    check(!DmKeyStore.validName("Identity"), "capitals")
    check(!DmKeyStore.validName(String(repeating: "a", count: 33)), "33 characters")
    check(!DmKeyStore.validName("../x"), "a path")
    check(!DmKeyStore.validName("é"), "not ASCII")

    // What a read's status means.
    check((try? DmKeyStore.outcome(errSecItemNotFound, nil)) == .some(nil), "absent is nil")
    check(
      (try? DmKeyStore.outcome(errSecSuccess, Data([1, 2]) as CFData)) == Data([1, 2]),
      "the bytes")
    check(
      failure { _ = try DmKeyStore.outcome(errSecDecode, nil) }?.code == "unreadable",
      "something there that can't be read is unreadable, never absent")
    check(
      failure { _ = try DmKeyStore.outcome(errSecSuccess, nil) }?.code == "unreadable",
      "no bytes is unreadable")
    check(
      failure { _ = try DmKeyStore.outcome(errSecInteractionNotAllowed, nil) }?.code == "failed",
      "locked before the first unlock is failed")

    // This binary, signed ad hoc: no group, so nothing can be kept.
    check(DmKeyStore.entitledGroup() == nil, "an ad hoc build has no keychain group")
    let service = "com.claktory.dm-keys.check-\(getpid())"
    let store = DmKeyStore(group: DmKeyStore.entitledGroup(), dataProtection: true, service: service)
    check(!store.available(), "unavailable without a group")
    check(failure { try store.put("identity", Data(count: 32)) }?.code == "failed", "put fails")
    check(failure { _ = try store.get("identity") }?.code == "failed", "get fails")

    // Named a group it isn't entitled to: the data-protection keychain
    // refuses (-34018), and it's unavailable, never kept elsewhere.
    let foreign = DmKeyStore(group: "ABCDE12345.dev.factory.app", dataProtection: true, service: service)
    check(!foreign.available(), "a group it isn't entitled to is unavailable")
    check(failure { try foreign.put("identity", Data(count: 32)) }?.code == "failed", "put fails")

    // Too much, or a bad name, fails before the keychain.
    check(
      failure { try foreign.put("identity", Data(count: DmKeyStore.valueLimit + 1)) }?.code
        == "failed", "over 4 KiB")
    check(failure { try foreign.put("Bad", Data(count: 1)) }?.code == "failed", "a bad name")

    // Hostile test 12: nothing of its service in the login keychain, nor in
    // user defaults.
    let login: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecMatchLimit as String: kSecMatchLimitAll,
      kSecReturnAttributes as String: true,
    ]
    var found: CFTypeRef?
    check(
      SecItemCopyMatching(login as CFDictionary, &found) == errSecItemNotFound,
      "nothing in the login keychain")
    check(
      !UserDefaults.standard.dictionaryRepresentation().keys.contains { $0.contains("dm") },
      "nothing in user defaults")

    if failures == 0 { print("DmKeysCheck: all passed") }
    exit(failures == 0 ? 0 : 1)
  }
}
