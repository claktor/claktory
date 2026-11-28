// Checks BackendPeer (Runner/BackendPeer.swift) without the app:
//
//   cd app/macos && swiftc -o /tmp/backend-peer-check \
//     Runner/BackendPeer.swift RunnerTests/BackendPeerCheck.swift \
//     && /tmp/backend-peer-check [path/to/a/released/factoryd]
//
// It listens on a Unix socket of its own, in a fresh temporary folder, and
// connects to it, so the peer it checks is itself: built here, ad hoc, it's
// no Claktory backend, however its user matches. Given a released factoryd,
// it also checks that one's signature meets the requirement for its team
// (reading the file only; nothing is run).
import Foundation
import Security

@main
enum BackendPeerCheck {
  static func main() throws {
    var failures = 0
    func check(_ ok: Bool, _ what: String, line: Int = #line) {
      if !ok {
        failures += 1
        print("FAILED (line \(line)): \(what)")
      }
    }

    // A server and a client on a socket of this process's own.
    let folder = FileManager.default.temporaryDirectory
      .appendingPathComponent("backend-peer-\(UUID().uuidString.prefix(8))")
    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: folder) }
    let path = folder.appendingPathComponent("app.sock").path
    func address() -> sockaddr_un {
      var address = sockaddr_un()
      address.sun_family = sa_family_t(AF_UNIX)
      withUnsafeMutableBytes(of: &address.sun_path) { raw in
        let bytes = Array(path.utf8)
        for (i, byte) in bytes.enumerated() { raw[i] = byte }
      }
      return address
    }
    let server = socket(AF_UNIX, SOCK_STREAM, 0)
    var at = address()
    let bound = withUnsafePointer(to: &at) {
      $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
        bind(server, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
      }
    }
    check(bound == 0 && listen(server, 1) == 0, "listening")
    let client = socket(AF_UNIX, SOCK_STREAM, 0)
    let connected = withUnsafePointer(to: &at) {
      $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
        connect(client, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
      }
    }
    check(connected == 0, "connected")
    defer {
      close(client)
      close(server)
    }

    func option(_ name: Int32, _ size: Int) -> Data {
      var data = Data(count: size)
      var length = socklen_t(size)
      let status = data.withUnsafeMutableBytes {
        getsockopt(client, SOL_LOCAL, name, $0.baseAddress, &length)
      }
      check(status == 0, "getsockopt \(name)")
      return data.prefix(Int(length))
    }
    let credentials = option(LOCAL_PEERCRED, MemoryLayout<xucred>.size)
    let token = option(LOCAL_PEERTOKEN, MemoryLayout<audit_token_t>.size)

    // Ad hoc (Claktory Dev): only the user is checked.
    check(BackendPeer.check(credentials: credentials, token: token, team: nil) == nil,
      "an ad hoc app talks to a backend of its own user")
    check(
      BackendPeer.check(credentials: credentials, token: token, team: nil, uid: getuid() + 1)?
        .contains("another user") == true,
      "but not to another user's")
    check(BackendPeer.check(credentials: Data(), token: token, team: nil) != nil,
      "nor to one whose credentials can't be read")

    // Signed by a team: this ad hoc process isn't Claktory's backend.
    let why = BackendPeer.check(credentials: credentials, token: token, team: "ABCDE12345")
    check(why?.contains("isn't Claktory's backend") == true,
      "a team's app refuses an ad hoc peer (\(why ?? "let in"))")
    check(BackendPeer.checkSignature(token: Data(count: 4), team: "ABCDE12345") != nil,
      "and a token that isn't one")
    var requirement: SecRequirement?
    check(
      SecRequirementCreateWithString(
        BackendPeer.requirement(team: "ABCDE12345") as CFString, [], &requirement)
        == errSecSuccess, "the requirement reads")

    // The home folder from the user database, whatever $HOME says.
    setenv("HOME", "/tmp/not-home", 1)
    let home = BackendPeer.userHome()
    check(home != nil && home != "/tmp/not-home", "the home ignores $HOME (\(home ?? "none"))")
    check(home == FileManager.default.homeDirectory(forUser: NSUserName())?.path,
      "and is the user's own")

    // The temporary folder from confstr, whatever $TMPDIR says.
    let realTemp = ProcessInfo.processInfo.environment["TMPDIR"]
    setenv("TMPDIR", "/tmp/not-temp/", 1)
    let temp = BackendPeer.userTemp()
    check(temp != nil && temp != "/tmp/not-temp/", "the temporary folder ignores $TMPDIR")
    if let realTemp {
      check(temp == realTemp, "and is the user's own (\(temp ?? "none"), \(realTemp))")
    }

    // A released backend meets the requirement for its own team, and not
    // for another.
    if CommandLine.arguments.count > 1 {
      let url = URL(fileURLWithPath: CommandLine.arguments[1])
      var code: SecStaticCode?
      check(SecStaticCodeCreateWithPath(url as CFURL, [], &code) == errSecSuccess, "read it")
      if let code {
        var info: CFDictionary?
        SecCodeCopySigningInformation(
          code, SecCSFlags(rawValue: kSecCSSigningInformation), &info)
        let team = (info as? [String: Any])?[kSecCodeInfoTeamIdentifier as String] as? String
        check(team != nil, "the release names its team")
        func meets(_ team: String) -> Bool {
          var requirement: SecRequirement?
          SecRequirementCreateWithString(
            BackendPeer.requirement(team: team) as CFString, [], &requirement)
          return SecStaticCodeCheckValidity(code, [], requirement) == errSecSuccess
        }
        check(meets(team ?? ""), "the release meets its team's requirement")
        check(!meets("ABCDE12345"), "and not another team's")
        check(BackendPeer.checkFlags(code) == nil, "hardened, not for debugging")
      }
    }

    if failures > 0 {
      print("\(failures) failed")
      exit(1)
    }
    print("ok")
  }
}
