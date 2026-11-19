import Foundation

/// Factory's app ran in the macOS sandbox until its remote updates came
/// (2026-11): its user defaults (the window's frame, unsent drafts, the
/// Factories it connects to) were in the sandbox's container. Unsandboxed,
/// the app reads ~/Library/Preferences instead, so on its first launch it
/// brings over what the container has, once.
enum SandboxMigration {
  /// Set once the container's defaults were brought over (or there were
  /// none), so it happens only once.
  static let doneKey = "MigratedFromSandbox"

  /// Where the sandboxed app with `bundleID` kept its defaults.
  static func containerDefaults(bundleID: String, home: URL) -> URL {
    home.appendingPathComponent(
      "Library/Containers/\(bundleID)/Data/Library/Preferences/\(bundleID).plist")
  }

  /// Copies each key of the plist at `from` that `defaults` doesn't have yet
  /// into it, unless that was done before; answers the keys it copied.
  @discardableResult
  static func run(from: URL, into defaults: UserDefaults) -> [String] {
    if defaults.bool(forKey: doneKey) { return [] }
    var copied: [String] = []
    if let old = NSDictionary(contentsOf: from) as? [String: Any] {
      for (key, value) in old where defaults.object(forKey: key) == nil {
        defaults.set(value, forKey: key)
        copied.append(key)
      }
    }
    defaults.set(true, forKey: doneKey)
    return copied.sorted()
  }

  /// This app's migration, at launch, before anything reads its defaults.
  static func runForThisApp() {
    guard let bundleID = Bundle.main.bundleIdentifier else { return }
    let home = FileManager.default.homeDirectoryForCurrentUser
    run(from: containerDefaults(bundleID: bundleID, home: home), into: .standard)
  }
}
