import Foundation
import Security

/// The app of the version this Mac's Claktory went back to when an update
/// failed (lib/older_app.dart). That version's backend has no socket for
/// this newer app, so the app offers to put that version's own app,
/// `<home>/releases/<version>/Factory.app`, in its place. Agents can write
/// releases/, so the app is first copied into a folder of its own beside
/// this one, only this user's, and that copy is checked, moved into this
/// app's place and checked again there: Claktory's app signed by this app's
/// own team (Developer ID, the hardened runtime, no get-task-allow), of
/// exactly that version.
/// Claktory Dev, signed ad hoc, takes only Claktory Dev's app.
enum OlderApp {
  static let identifier = "dev.factory.app"
  static let devIdentifier = "dev.factory.app.dev"

  struct Refused: Error, CustomStringConvertible {
    let description: String
    init(_ why: String) { description = why }
  }

  /// "0.1.27" for a release's "v0.1.27", or with [dev], also for Claktory
  /// Dev's "v0.1.27-dev.20261127-2244.09"; nil for anything else.
  static func numbers(_ version: String, dev: Bool) -> String? {
    let release = #"^v([0-9]{1,6}\.[0-9]{1,6}\.[0-9]{1,6})$"#
    let devRelease = #"^v([0-9]{1,6}\.[0-9]{1,6}\.[0-9]{1,6})-dev\.[0-9]{8}-[0-9]{4}\.[0-9]{2}$"#
    for pattern in dev ? [release, devRelease] : [release] {
      let expression = try! NSRegularExpression(pattern: pattern)
      let whole = NSRange(version.startIndex..., in: version)
      if let match = expression.firstMatch(in: version, range: whole),
        let numbers = Range(match.range(at: 1), in: version)
      {
        return String(version[numbers])
      }
    }
    return nil
  }

  /// What the app's signature must meet: Claktory's, signed with a
  /// Developer ID of [team]; with no team, Claktory Dev's.
  static func requirement(team: String?) -> String {
    guard let team else { return "identifier \"\(devIdentifier)\"" }
    return "anchor apple generic and identifier \"\(identifier)\""
      + " and certificate 1[field.1.2.840.113635.100.6.2.6] exists"
      + " and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"
      + " and certificate leaf[subject.OU] = \"\(team)\""
  }

  /// Nil when [app] is [version]'s app, signed as [requirement] says for
  /// [team] (nil: Claktory Dev's); otherwise why not.
  static func check(_ app: URL, version: String, team: String?) -> String? {
    guard let numbers = numbers(version, dev: team == nil) else {
      return "\(version) isn't a version"
    }
    var code: SecStaticCode?
    guard SecStaticCodeCreateWithPath(app as CFURL, [], &code) == errSecSuccess, let code
    else { return "its signature can't be read" }
    var requirement: SecRequirement?
    guard
      SecRequirementCreateWithString(Self.requirement(team: team) as CFString, [], &requirement)
        == errSecSuccess, let requirement
    else { return "the requirement can't be made" }
    let strictly = SecCSFlags(
      rawValue: kSecCSStrictValidate | kSecCSCheckAllArchitectures | kSecCSCheckNestedCode)
    let status = SecStaticCodeCheckValidity(code, strictly, requirement)
    guard status == errSecSuccess else {
      return team == nil
        ? "it isn't Claktory Dev's app (\(status))"
        : "it isn't Claktory's app, signed by its developer (\(status))"
    }
    if team != nil, let why = BackendPeer.checkFlags(code) { return why }
    var info: CFDictionary?
    guard
      SecCodeCopySigningInformation(code, SecCSFlags(rawValue: kSecCSSigningInformation), &info)
        == errSecSuccess,
      let info = info as? [String: Any],
      let plist = info[kSecCodeInfoPList as String] as? [String: Any]
    else { return "its Info.plist can't be read" }
    let shown = plist["CFBundleShortVersionString"] as? String ?? ""
    guard shown == numbers else { return "it's version \(shown), not \(version)" }
    return nil
  }

  /// Copies [version]'s app from [home]'s releases into a new folder
  /// beside [installed], only this user's, and checks the copy; answers the
  /// copy. Nothing is left behind when it throws.
  static func stage(home: String, version: String, installed: URL, team: String?) throws -> URL {
    guard numbers(version, dev: team == nil) != nil else {
      throw Refused("\(version) isn't a version")
    }
    let name = team == nil ? "Factory Dev.app" : "Factory.app"
    let source = URL(fileURLWithPath: home, isDirectory: true)
      .appendingPathComponent("releases", isDirectory: true)
      .appendingPathComponent(version, isDirectory: true)
      .appendingPathComponent(name, isDirectory: true)
    var folder: ObjCBool = false
    guard FileManager.default.fileExists(atPath: source.path, isDirectory: &folder), folder.boolValue
    else {
      throw Refused("\(version)'s app isn't on this Mac any more")
    }
    let parent = installed.deletingLastPathComponent()
    guard FileManager.default.isWritableFile(atPath: parent.path) else {
      throw Refused("this app's folder, \(parent.path), can't be changed")
    }
    let staging = parent.appendingPathComponent(".claktory-\(version)-\(UUID().uuidString)")
    try FileManager.default.createDirectory(
      at: staging, withIntermediateDirectories: false,
      attributes: [.posixPermissions: 0o700])
    let copy = staging.appendingPathComponent(name, isDirectory: true)
    do {
      try run("/usr/bin/ditto", [source.path, copy.path])
      if let why = check(copy, version: version, team: team) { throw Refused(why) }
    } catch {
      try? FileManager.default.removeItem(at: staging)
      throw error
    }
    return copy
  }

  /// Puts [staged] in [installed]'s place, the newer app moved aside to
  /// `.previous`, and checks it again there: staging is a folder anyone
  /// running as this user can change, so the copy that counts is the one
  /// in place. One that fails comes out, and the newer app goes back.
  static func install(_ staged: URL, at installed: URL, version: String, team: String?) throws {
    let files = FileManager.default
    let previous = URL(fileURLWithPath: installed.path + ".previous")
    try? files.removeItem(at: previous)
    try files.moveItem(at: installed, to: previous)
    do {
      try files.moveItem(at: staged, to: installed)
    } catch {
      try? files.moveItem(at: previous, to: installed)
      throw error
    }
    if let why = check(installed, version: version, team: team) {
      try? files.removeItem(at: installed)
      try? files.moveItem(at: previous, to: installed)
      throw Refused(why)
    }
  }

  /// Puts the newer app back in place of [installed], as [install] found
  /// it.
  static func putBack(_ installed: URL) {
    let previous = URL(fileURLWithPath: installed.path + ".previous")
    guard FileManager.default.fileExists(atPath: previous.path) else { return }
    try? FileManager.default.removeItem(at: installed)
    try? FileManager.default.moveItem(at: previous, to: installed)
  }

  /// Once process [pid] (this one) is gone, deletes the newer app
  /// [install] moved aside and opens [installed] with [opener]; Claktory
  /// Dev in the background.
  @discardableResult
  static func relaunch(
    _ installed: URL, dev: Bool,
    pid: Int32 = ProcessInfo.processInfo.processIdentifier, opener: String = "/usr/bin/open"
  ) throws -> Process {
    let script = """
      app="$1"; pid="$2"; opener="$3"
      for _ in $(/usr/bin/seq 100); do /bin/kill -0 "$pid" 2>/dev/null || break; /bin/sleep 0.1; done
      /bin/kill "$pid" 2>/dev/null
      /bin/rm -rf "$app.previous"
      "$opener" \(dev ? "-g " : "")"$app"
      """
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/bin/sh")
    process.arguments = ["-c", script, "sh", installed.path, "\(pid)", opener]
    try process.run()
    return process
  }

  /// This app's team, for a build signed by one; nil for Claktory Dev, or
  /// a refusal for any other build signed ad hoc.
  static func ownTeam() throws -> String? {
    if let team = BackendPeer.ownTeam() { return team }
    guard Bundle.main.bundleIdentifier != identifier else {
      throw Refused("this app isn't signed by its developer")
    }
    return nil
  }

  /// [version]'s app in place of this one, which then quits.
  static func use(home devHome: String, version: String) throws {
    let team = try ownTeam()
    let home: String
    if team == nil {
      home = devHome
    } else if let user = BackendPeer.userHome() {
      home = user + "/.factory"
    } else {
      throw Refused("this Mac doesn't say where your home folder is")
    }
    let installed = Bundle.main.bundleURL
    let staged = try stage(home: home, version: version, installed: installed, team: team)
    defer { try? FileManager.default.removeItem(at: staged.deletingLastPathComponent()) }
    try install(staged, at: installed, version: version, team: team)
    do {
      try relaunch(installed, dev: team == nil)
    } catch {
      putBack(installed)
      throw error
    }
  }

  private static func run(_ path: String, _ arguments: [String]) throws {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: path)
    process.arguments = arguments
    let err = Pipe()
    process.standardError = err
    process.standardOutput = FileHandle.nullDevice
    try process.run()
    let message = err.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    if process.terminationStatus != 0 {
      throw Refused(
        "it couldn't be copied (\(String(decoding: message, as: UTF8.self)))")
    }
  }
}
