// Checks SetupLink.swift's request file rules (#245) without the app:
//
//   cd app/macos && swiftc -parse-as-library -o /tmp/setup-link-check \
//     Runner/SetupLink.swift RunnerTests/SetupLinkCheck.swift \
//     && /tmp/setup-link-check
//
// It works in a temporary folder of its own, never the app's.
import Darwin
import Foundation

@main
enum SetupLinkCheck {
  static func main() {
    var failures = 0
    func check(_ ok: Bool, _ what: String, line: Int = #line) {
      if !ok {
        failures += 1
        print("FAILED (line \(line)): \(what)")
      }
    }

    let folder = URL(fileURLWithPath: NSTemporaryDirectory())
      .appendingPathComponent("setup-link-check-\(getpid())", isDirectory: true)
    try! FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: folder) }
    let request = folder.appendingPathComponent(SetupLink.requestName)
    let code = "factory://?t=00&name=box&e=4fb3"

    /// Writes a request as an agent would: [mode], [body], [age] seconds old.
    func write(_ body: String, mode: mode_t = 0o600, age: TimeInterval = 0) {
      unlink(request.path)
      FileManager.default.createFile(
        atPath: request.path, contents: Data(body.utf8), attributes: [.posixPermissions: mode])
      if age != 0 {
        let then = Date().addingTimeInterval(-age)
        try! FileManager.default.setAttributes([.modificationDate: then], ofItemAtPath: request.path)
      }
    }

    func exists() -> Bool { FileManager.default.fileExists(atPath: request.path) }

    // None there.
    check(SetupLink.read(request) == .none, "no request reads as none")

    // A good one: its code, and it's gone.
    write(#"{"code": " \#(code) "}"#)
    check(SetupLink.read(request) == .code(code), "a good request gives its code, trimmed")
    check(!exists(), "a request is removed once read")

    // Readable by others.
    write(#"{"code": "\#(code)"}"#, mode: 0o644)
    check(SetupLink.read(request) == .refused("readable_by_others"), "mode 0644 is refused")
    check(!exists(), "a refused request is removed too")

    // Too old.
    write(#"{"code": "\#(code)"}"#, age: 11 * 60)
    check(SetupLink.read(request) == .refused("too_old"), "eleven minutes old is refused")
    write(#"{"code": "\#(code)"}"#, age: -5 * 60)
    check(SetupLink.read(request) == .refused("too_old"), "dated five minutes ahead is refused")
    write(#"{"code": "\#(code)"}"#, age: -20)
    check(SetupLink.read(request) == .code(code), "a clock twenty seconds ahead is fine")
    write(#"{"code": "\#(code)"}"#, age: 9 * 60)
    check(SetupLink.read(request) == .code(code), "nine minutes old is taken")

    // No code, or not JSON.
    write(#"{"name": "box"}"#)
    check(SetupLink.read(request) == .refused("no_code"), "no code is refused")
    write("factory://?t=00")
    check(SetupLink.read(request) == .refused("no_code"), "a bare code (not JSON) is refused")

    // Too big.
    write(#"{"code": "\#(String(repeating: "x", count: SetupLink.maxSize))"}"#)
    check(SetupLink.read(request) == .refused("too_big"), "a huge request is refused")

    // A link to a file of the right kind is still a link.
    let elsewhere = folder.appendingPathComponent("elsewhere.json")
    FileManager.default.createFile(
      atPath: elsewhere.path, contents: Data(#"{"code": "\#(code)"}"#.utf8),
      attributes: [.posixPermissions: 0o600])
    unlink(request.path)
    symlink(elsewhere.path, request.path)
    check(SetupLink.read(request) == .refused("not_a_file"), "a symbolic link is refused")
    check(!exists(), "the link is removed")
    check(FileManager.default.fileExists(atPath: elsewhere.path), "what it pointed to is left alone")

    // A folder in its place.
    unlink(request.path)
    mkdir(request.path, 0o700)
    check(SetupLink.read(request) == .refused("not_a_file"), "a folder is refused")
    rmdir(request.path)

    // The status: whole, this user's only.
    SetupLink.writeStatus(["state": "waiting", "number": "1234"], in: folder)
    let status = folder.appendingPathComponent(SetupLink.statusName)
    let written = try! JSONSerialization.jsonObject(with: Data(contentsOf: status)) as! [String: Any]
    check(written["state"] as? String == "waiting", "the status says its state")
    check(written["number"] as? String == "1234", "and what came with it")
    check(written["at_ms"] is NSNumber, "and when")
    let mode = (try! FileManager.default.attributesOfItem(atPath: status.path)[.posixPermissions]
      as! NSNumber).intValue
    check(mode == 0o600, "the status is readable only by this user")
    SetupLink.writeStatus(["state": "connected", "name": "box"], in: folder)
    let replaced = try! JSONSerialization.jsonObject(with: Data(contentsOf: status)) as! [String: Any]
    check(replaced["state"] as? String == "connected", "a new status replaces the old")
    check(replaced["number"] == nil, "whole")
    let left = try! FileManager.default.contentsOfDirectory(atPath: folder.path)
    check(!left.contains { $0.hasPrefix(".") }, "no temporary file is left")

    if failures == 0 {
      print("SetupLink: all checks passed")
    } else {
      print("SetupLink: \(failures) failed")
      exit(1)
    }
  }
}
