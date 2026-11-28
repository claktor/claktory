import Foundation
import Security

/// Who listens at app.sock (#546 A, Fugo's layer 2). Before the app says a
/// word on its socket, it checks that the process at the other end is this
/// Mac's own Claktory backend. Otherwise another program of the person's
/// that took the socket's place could read what they type there (secrets,
/// the vault's password). The app's Dart side reads the peer's credentials
/// and audit token off the connection (getsockopt's LOCAL_PEERCRED and
/// LOCAL_PEERTOKEN) and asks here (MainFlutterWindow's checkBackendPeer).
enum BackendPeer {
  /// The backend's code-signing identifier (scripts/sign-release.sh).
  static let identifier = "dev.factory.factoryd"

  /// SecCodeSignatureFlags' hardened runtime.
  static let runtimeFlag: UInt32 = 0x10000

  /// What the backend's signature must meet: Claktory's own, signed with
  /// a Developer ID of [team] (this app's own team), as its designated
  /// requirement reads.
  static func requirement(team: String) -> String {
    "identifier \"\(identifier)\" and anchor apple generic"
      + " and certificate 1[field.1.2.840.113635.100.6.2.6] exists"
      + " and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"
      + " and certificate leaf[subject.OU] = \"\(team)\""
  }

  /// Nil when the peer whose LOCAL_PEERCRED is [credentials] (an xucred)
  /// and LOCAL_PEERTOKEN [token] (an audit_token_t) may be talked to;
  /// otherwise why not, in a few words for the log. Every build checks the
  /// peer runs as this user. One signed by a team ([team]) also checks it's
  /// Claktory's backend signed by that team, with the hardened runtime and
  /// no get-task-allow. An ad hoc build (Claktory Dev) checks only the
  /// user, its backend being ad hoc too.
  static func check(credentials: Data, token: Data, team: String?, uid: uid_t = getuid())
    -> String?
  {
    guard credentials.count >= 8 else { return "its credentials can't be read" }
    let version = credentials.withUnsafeBytes {
      $0.loadUnaligned(fromByteOffset: 0, as: UInt32.self)
    }
    let peer = credentials.withUnsafeBytes {
      $0.loadUnaligned(fromByteOffset: 4, as: UInt32.self)
    }
    guard version == UInt32(XUCRED_VERSION) else { return "its credentials can't be read" }
    guard peer == uid else { return "it runs as another user (\(peer))" }
    guard let team, !team.isEmpty else { return nil }
    return checkSignature(token: token, team: team)
  }

  /// Nil when the process whose audit token is [token] is Claktory's
  /// backend signed by [team]; otherwise why not.
  static func checkSignature(token: Data, team: String) -> String? {
    guard token.count == MemoryLayout<audit_token_t>.size else {
      return "its audit token can't be read"
    }
    var guest: SecCode?
    let attributes = [kSecGuestAttributeAudit: token] as CFDictionary
    var status = SecCodeCopyGuestWithAttributes(nil, attributes, [], &guest)
    guard status == errSecSuccess, let guest else {
      return "its code can't be found (\(status))"
    }
    var requirement: SecRequirement?
    status = SecRequirementCreateWithString(
      Self.requirement(team: team) as CFString, [], &requirement)
    guard status == errSecSuccess, let requirement else {
      return "the requirement can't be made (\(status))"
    }
    status = SecCodeCheckValidity(guest, [], requirement)
    guard status == errSecSuccess else {
      return "it isn't Claktory's backend (\(status))"
    }
    var code: SecStaticCode?
    guard SecCodeCopyStaticCode(guest, [], &code) == errSecSuccess, let code else {
      return "its signature can't be read"
    }
    return checkFlags(code)
  }

  /// Nil when [code] is signed with the hardened runtime and without
  /// get-task-allow (a debugger's way in); otherwise why not.
  static func checkFlags(_ code: SecStaticCode) -> String? {
    var info: CFDictionary?
    let flags = SecCSFlags(
      rawValue: kSecCSSigningInformation | kSecCSRequirementInformation)
    guard SecCodeCopySigningInformation(code, flags, &info) == errSecSuccess,
      let info = info as? [String: Any]
    else { return "its signature can't be read" }
    let signed = (info[kSecCodeInfoFlags as String] as? NSNumber)?.uint32Value ?? 0
    guard signed & runtimeFlag != 0 else {
      return "it isn't signed with the hardened runtime"
    }
    let entitlements = info[kSecCodeInfoEntitlementsDict as String] as? [String: Any] ?? [:]
    if entitlements["com.apple.security.get-task-allow"] as? Bool == true {
      return "it's signed for debugging"
    }
    return nil
  }

  /// The person's home folder from the user database (getpwuid_r), never
  /// $HOME, which another program could set (launchctl setenv) to point the
  /// app at a home of its own (Fugo); nil when the database says nothing.
  static func userHome() -> String? {
    var entry = passwd()
    var found: UnsafeMutablePointer<passwd>?
    let most = sysconf(Int32(_SC_GETPW_R_SIZE_MAX))
    var buffer = [CChar](repeating: 0, count: most > 0 ? Int(most) : 16384)
    guard getpwuid_r(getuid(), &entry, &buffer, buffer.count, &found) == 0,
      found != nil, let dir = entry.pw_dir
    else { return nil }
    let home = String(cString: dir)
    return home.hasPrefix("/") ? home : nil
  }

  /// This user's own temporary folder (confstr's DARWIN_USER_TEMP_DIR), as
  /// factoryd's authority::socket_path takes it for a socket whose path
  /// would be too long, never $TMPDIR; nil when it says nothing.
  static func userTemp() -> String? {
    var buffer = [CChar](repeating: 0, count: 1024)
    let length = confstr(_CS_DARWIN_USER_TEMP_DIR, &buffer, buffer.count)
    guard length > 1, length <= buffer.count else { return nil }
    let folder = String(cString: buffer)
    return folder.hasPrefix("/") ? folder : nil
  }

  /// This app's own team, from its running signature; nil when it's signed
  /// ad hoc.
  static func ownTeam() -> String? {
    var running: SecCode?
    var code: SecStaticCode?
    var info: CFDictionary?
    guard SecCodeCopySelf([], &running) == errSecSuccess, let running,
      SecCodeCopyStaticCode(running, [], &code) == errSecSuccess, let code,
      SecCodeCopySigningInformation(
        code, SecCSFlags(rawValue: kSecCSSigningInformation), &info) == errSecSuccess,
      let info = info as? [String: Any],
      let team = info[kSecCodeInfoTeamIdentifier as String] as? String, !team.isEmpty
    else { return nil }
    return team
  }
}
