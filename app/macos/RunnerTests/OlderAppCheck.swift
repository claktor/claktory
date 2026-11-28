// Checks OlderApp (Runner/OlderApp.swift) without the app:
//
//   cd app/macos && swiftc -o /tmp/older-app-check \
//     Runner/BackendPeer.swift Runner/OlderApp.swift RunnerTests/OlderAppCheck.swift \
//     && /tmp/older-app-check [path/to/a/released/Factory.app]
//
// It makes small apps of its own in a fresh temporary folder, signed ad hoc
// (codesign), and checks what OlderApp takes and refuses: only a version
// exactly, Claktory Dev's app for an ad hoc build and never for a team's,
// the version its Info.plist says, and nothing changed after signing, both
// in its staging folder and once in the app's place. Given
// a released Factory.app (read only: it's copied, never run), it also
// checks that one passes for its own team and version, and fails for
// another team, another version, or once its copy is changed.
import Foundation
import Security

@main
enum OlderAppCheck {
  static func main() throws {
    var failures = 0
    func check(_ ok: Bool, _ what: String, line: Int = #line) {
      if !ok {
        failures += 1
        print("FAILED (line \(line)): \(what)")
      }
    }
    let files = FileManager.default
    let folder = files.temporaryDirectory
      .appendingPathComponent("older-app-\(UUID().uuidString.prefix(8))")
    try files.createDirectory(at: folder, withIntermediateDirectories: true)
    defer { try? files.removeItem(at: folder) }

    func run(_ path: String, _ arguments: [String]) {
      let process = Process()
      process.executableURL = URL(fileURLWithPath: path)
      process.arguments = arguments
      process.standardOutput = FileHandle.nullDevice
      process.standardError = FileHandle.nullDevice
      try! process.run()
      process.waitUntilExit()
      precondition(process.terminationStatus == 0, "\(path) \(arguments) failed")
    }
    /// An app at [url], signed ad hoc as [identifier], saying [version].
    func app(_ url: URL, identifier: String, version: String) -> URL {
      let contents = url.appendingPathComponent("Contents")
      try! files.createDirectory(
        at: contents.appendingPathComponent("MacOS"), withIntermediateDirectories: true)
      try! files.createDirectory(
        at: contents.appendingPathComponent("Resources"), withIntermediateDirectories: true)
      let plist: [String: Any] = [
        "CFBundleIdentifier": identifier,
        "CFBundleExecutable": "app",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": version,
      ]
      try! PropertyListSerialization.data(fromPropertyList: plist, format: .xml, options: 0)
        .write(to: contents.appendingPathComponent("Info.plist"))
      try! files.copyItem(
        atPath: "/usr/bin/true", toPath: contents.appendingPathComponent("MacOS/app").path)
      try! Data("a resource".utf8).write(to: contents.appendingPathComponent("Resources/words"))
      run("/usr/bin/codesign", ["--force", "--sign", "-", "--identifier", identifier, url.path])
      return url
    }

    // Only a version exactly: nothing that could name another folder.
    check(OlderApp.numbers("v0.1.27", dev: false) == "0.1.27", "a release's version")
    for bad in ["v0.1.27/..", "../v0.1.27", "v0.1", "0.1.27", "v0.1.27 ", "v0.1.27-dev.20261127-2244.09"] {
      check(OlderApp.numbers(bad, dev: false) == nil, "\(bad) isn't a release's version")
    }
    check(
      OlderApp.numbers("v0.1.27-dev.20261127-2244.09", dev: true) == "0.1.27",
      "Claktory Dev's own versions")
    check(OlderApp.numbers("v0.1.27-dev.x/../..", dev: true) == nil, "nor a dev one's")

    let dev = "v0.1.27-dev.20261127-2244.09"
    let devApp = app(
      folder.appendingPathComponent("Factory Dev.app"), identifier: OlderApp.devIdentifier,
      version: "0.1.27")
    check(OlderApp.check(devApp, version: dev, team: nil) == nil, "Claktory Dev's own app")
    check(
      OlderApp.check(devApp, version: "v0.1.26-dev.20261127-2244.09", team: nil) != nil,
      "another version than its Info.plist says")
    let other = app(
      folder.appendingPathComponent("Other.app"), identifier: "dev.factory.other",
      version: "0.1.27")
    check(OlderApp.check(other, version: dev, team: nil) != nil, "another identifier")
    // Ad hoc, as Claktory's own identifier: never for a team's build.
    let adHoc = app(
      folder.appendingPathComponent("Factory.app"), identifier: OlderApp.identifier,
      version: "0.1.27")
    check(OlderApp.check(adHoc, version: "v0.1.27", team: "ABCDE12345") != nil, "an ad hoc app")

    // Staged beside the installed app, in a folder only this user opens.
    let home = folder.appendingPathComponent("home")
    let release = home.appendingPathComponent("releases/\(dev)")
    try files.createDirectory(at: release, withIntermediateDirectories: true)
    try files.copyItem(at: devApp, to: release.appendingPathComponent("Factory Dev.app"))
    let applications = folder.appendingPathComponent("Applications")
    try files.createDirectory(at: applications, withIntermediateDirectories: true)
    let installed = applications.appendingPathComponent("Claktory Dev.app")
    let staged = try OlderApp.stage(
      home: home.path, version: dev, installed: installed, team: nil)
    let staging = staged.deletingLastPathComponent()
    check(staging.deletingLastPathComponent().path == applications.path, "staged beside it")
    let mode = try files.attributesOfItem(atPath: staging.path)[.posixPermissions] as? Int
    check(mode == 0o700, "its folder only this user's (\(String(mode ?? 0, radix: 8)))")
    check(OlderApp.check(staged, version: dev, team: nil) == nil, "the copy passes")

    // Checked again once in the app's place: a copy changed after its
    // first check comes out, and the newer app is back as it was.
    _ = app(installed, identifier: OlderApp.devIdentifier, version: "0.2.0")
    func shownVersion(_ app: URL) -> String? {
      NSDictionary(contentsOf: app.appendingPathComponent("Contents/Info.plist"))?[
        "CFBundleShortVersionString"] as? String
    }
    let changed = try OlderApp.stage(
      home: home.path, version: dev, installed: installed, team: nil)
    try Data("changed".utf8).write(to: changed.appendingPathComponent("Contents/Resources/words"))
    check(
      (try? OlderApp.install(changed, at: installed, version: dev, team: nil)) == nil,
      "a copy changed after its check")
    check(shownVersion(installed) == "0.2.0", "the newer app back")
    check(!files.fileExists(atPath: installed.path + ".previous"), "nothing aside")
    try files.removeItem(at: changed.deletingLastPathComponent())

    // In place, then opened once the app has quit, the newer one gone.
    let swapping = try OlderApp.stage(
      home: home.path, version: dev, installed: installed, team: nil)
    try OlderApp.install(swapping, at: installed, version: dev, team: nil)
    try files.removeItem(at: swapping.deletingLastPathComponent())
    check(shownVersion(installed) == "0.1.27", "the older app in place")
    check(files.fileExists(atPath: installed.path + ".previous"), "the newer one aside")
    let running = Process()
    running.executableURL = URL(fileURLWithPath: "/bin/sleep")
    running.arguments = ["1"]
    try running.run()
    let relaunch = try OlderApp.relaunch(
      installed, dev: true, pid: running.processIdentifier, opener: "/usr/bin/true")
    check(files.fileExists(atPath: installed.path + ".previous"), "kept while the app runs")
    relaunch.waitUntilExit()
    check(!running.isRunning, "opened only once the app has quit")
    check(!files.fileExists(atPath: installed.path + ".previous"), "the newer one gone")
    check(OlderApp.check(installed, version: dev, team: nil) == nil, "the older app passes")
    // A bundle changed after it was signed doesn't.
    try Data("changed".utf8).write(to: staged.appendingPathComponent("Contents/Resources/words"))
    check(OlderApp.check(staged, version: dev, team: nil) != nil, "a changed copy")
    // Nothing staged when the release's app fails, nor when there's none.
    try Data("changed".utf8).write(
      to: release.appendingPathComponent("Factory Dev.app/Contents/Resources/words"))
    check(
      (try? OlderApp.stage(home: home.path, version: dev, installed: installed, team: nil))
        == nil, "a changed release's app")
    check(
      (try? OlderApp.stage(
        home: home.path, version: "v0.1.20-dev.20261127-2244.09", installed: installed,
        team: nil)) == nil, "a version not on this Mac")
    let left = try files.contentsOfDirectory(atPath: applications.path)
    check(
      Set(left) == [staging.lastPathComponent, installed.lastPathComponent],
      "nothing left but the app and the first copy: \(left)")

    // A released Factory.app: its own team's, of its own version.
    if CommandLine.arguments.count > 1 {
      let real = URL(fileURLWithPath: CommandLine.arguments[1])
      var code: SecStaticCode?
      var info: CFDictionary?
      SecStaticCodeCreateWithPath(real as CFURL, [], &code)
      SecCodeCopySigningInformation(
        code!, SecCSFlags(rawValue: kSecCSSigningInformation), &info)
      let signing = info as! [String: Any]
      let team = signing[kSecCodeInfoTeamIdentifier as String] as! String
      let plist = signing[kSecCodeInfoPList as String] as! [String: Any]
      let version = "v" + (plist["CFBundleShortVersionString"] as! String)
      let releaseHome = folder.appendingPathComponent("release-home")
      let releaseFolder = releaseHome.appendingPathComponent("releases/\(version)")
      try files.createDirectory(at: releaseFolder, withIntermediateDirectories: true)
      run("/usr/bin/ditto", [real.path, releaseFolder.appendingPathComponent("Factory.app").path])
      let copy = try OlderApp.stage(
        home: releaseHome.path, version: version,
        installed: applications.appendingPathComponent("Claktory.app"), team: team)
      check(OlderApp.check(copy, version: version, team: team) == nil, "the release passes")
      check(OlderApp.check(copy, version: "v0.0.1", team: team) != nil, "as another version")
      check(OlderApp.check(copy, version: version, team: "ABCDE12345") != nil, "another team")
      check(OlderApp.check(copy, version: version, team: nil) != nil, "as Claktory Dev's")
      let plistFile = copy.appendingPathComponent("Contents/Info.plist")
      var changed = try Data(contentsOf: plistFile)
      changed.append(contentsOf: Array("\n".utf8))
      try changed.write(to: plistFile)
      check(OlderApp.check(copy, version: version, team: team) != nil, "once changed")
    }

    if failures > 0 {
      print("\(failures) failed")
      exit(1)
    }
    print("All checks passed.")
  }
}
