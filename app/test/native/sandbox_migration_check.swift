// Checks SandboxMigration (macos/Runner/SandboxMigration.swift), compiled
// together with it by test/sandbox_migration_test.dart. Prints "ok", or
// what failed and exits 1.
import Foundation

func check(_ condition: Bool, _ what: String) {
  if !condition {
    print("failed: \(what)")
    exit(1)
  }
}

@main
struct SandboxMigrationCheck {
  static func main() {
    let folder = FileManager.default.temporaryDirectory
      .appendingPathComponent("factory-sandbox-migration-\(UUID().uuidString)")
    try! FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: folder) }

    // The sandboxed app's defaults, as its container had them.
    let home = folder
    let bundleID = "dev.factory.app.test"
    let plist = SandboxMigration.containerDefaults(bundleID: bundleID, home: home)
    check(
      plist.path.hasSuffix(
        "Library/Containers/dev.factory.app.test/Data/Library/Preferences/dev.factory.app.test.plist"),
      "the container's path")
    try! FileManager.default.createDirectory(
      at: plist.deletingLastPathComponent(), withIntermediateDirectories: true)
    let old: NSDictionary = [
      "NSWindow Frame Factory main window": "100 100 1440 900 0 0 3840 1575 ",
      "Drafts": ["builder": "half a thought"],
      "Connections": ["current": "factory://mac-mini:7443?t=x", "saved": ["factory://mac-mini:7443?t=x"]],
      "Kept": "from the container",
    ]
    check(old.write(to: plist, atomically: true), "writing the old defaults")

    let suite = "dev.factory.app.test.\(UUID().uuidString)"
    let defaults = UserDefaults(suiteName: suite)!
    defer { defaults.removePersistentDomain(forName: suite) }
    // Something the unsandboxed app already has stays as it is.
    defaults.set("already here", forKey: "Kept")

    let copied = SandboxMigration.run(from: plist, into: defaults)
    check(
      copied == ["Connections", "Drafts", "NSWindow Frame Factory main window"],
      "copied the missing keys only: \(copied)")
    check(defaults.string(forKey: "Kept") == "already here", "an existing key is kept")
    check(
      (defaults.dictionary(forKey: "Drafts") as? [String: String]) == ["builder": "half a thought"],
      "drafts came over")
    check(
      defaults.string(forKey: "NSWindow Frame Factory main window")
        == "100 100 1440 900 0 0 3840 1575 ",
      "the window's frame came over")
    check(
      (defaults.dictionary(forKey: "Connections")?["saved"] as? [String])
        == ["factory://mac-mini:7443?t=x"],
      "the saved Factories came over")
    check(defaults.bool(forKey: SandboxMigration.doneKey), "marked done")

    // Only once: a later change in the container isn't brought over again.
    defaults.removeObject(forKey: "Drafts")
    check(SandboxMigration.run(from: plist, into: defaults).isEmpty, "only once")
    check(defaults.object(forKey: "Drafts") == nil, "nothing copied the second time")

    // No container (a new install): nothing to copy, and done.
    let fresh = UserDefaults(suiteName: suite + ".fresh")!
    defer { fresh.removePersistentDomain(forName: suite + ".fresh") }
    let none = SandboxMigration.run(
      from: folder.appendingPathComponent("missing.plist"), into: fresh)
    check(none.isEmpty && fresh.bool(forKey: SandboxMigration.doneKey), "a new install")

    print("ok")
  }
}
