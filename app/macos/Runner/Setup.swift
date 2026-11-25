import Cocoa
import FlutterMacOS
import ServiceManagement

/// Setting Claktory up on this Mac from the app itself (app/lib/setup.dart):
/// the app downloaded as a disk image carries the backend in
/// Contents/Helpers, and `factoryd install` there puts it in place. Also
/// moving the app into Applications when it runs from the disk image, or
/// from where macOS put it aside (App Translocation).
enum Setup {
  static func register(with messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: "factory/setup", binaryMessenger: messenger)
    channel.setMethodCallHandler { call, result in
      switch call.method {
      case "location":
        let path = Bundle.main.bundlePath
        result([
          "path": path,
          "needsMove": needsMove(path),
          // Run from the disk image itself, rather than a copy macOS made.
          "fromImage": diskImage(of: originalURL) != nil,
          "helpers": FileManager.default.isExecutableFile(atPath: helper.path),
        ])
      case "install":
        let arguments = call.arguments as? [String] ?? []
        install(arguments, result)
      case "uninstall":
        let arguments = call.arguments as? [String] ?? []
        uninstall(arguments, result)
      case "removeApp":
        removeApp(forget: call.arguments as? Bool ?? false, result)
      case "moveToApplications":
        moveToApplications(result)
      case "backgroundStatus":
        result(backgroundStatus())
      case "openLoginItems":
        if #available(macOS 13.0, *) {
          SMAppService.openSystemSettingsLoginItems()
        } else {
          NSWorkspace.shared.open(
            URL(fileURLWithPath: "/System/Library/PreferencePanes/Accounts.prefPane"))
        }
        result(nil)
      case "appOnly":
        result(UserDefaults.standard.bool(forKey: appOnlyKey))
      case "setAppOnly":
        if call.arguments as? Bool ?? false {
          UserDefaults.standard.set(true, forKey: appOnlyKey)
        } else {
          UserDefaults.standard.removeObject(forKey: appOnlyKey)
        }
        result(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
    // Kept for as long as the app runs.
    channels.append(channel)
    Engine.register(with: messenger)
  }

  private static var channels: [FlutterMethodChannel] = []

  /// The LaunchAgent that runs this Mac's Claktory (launch_agent.rs's
  /// LABEL), which macOS lists under Login Items → Allow in the Background.
  static var launchAgent: URL {
    FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent("Library/LaunchAgents/dev.factory.factoryd.plist")
  }

  /// Whether macOS lets the LaunchAgent run (friend bug #28): "enabled";
  /// "requiresApproval" when its switch under Allow in the Background is
  /// off, so launchd won't start it; "notRegistered" or "notFound" when
  /// macOS knows no such job (not set up, or its plist is gone); and
  /// "unavailable" before macOS 13, which can't say.
  static func backgroundStatus() -> String {
    guard #available(macOS 13.0, *) else { return "unavailable" }
    switch SMAppService.statusForLegacyPlist(at: launchAgent) {
    case .enabled: return "enabled"
    case .requiresApproval: return "requiresApproval"
    case .notRegistered: return "notRegistered"
    case .notFound: return "notFound"
    @unknown default: return "unavailable"
    }
  }

  /// This Mac has no Claktory of its own: the app shows Claktories on
  /// other computers only. `install.sh --app-only` writes it
  /// (`defaults write dev.factory.app ClaktoryAppOnly -bool true`), and so
  /// does choosing "Use a Claktory on another computer" at first launch.
  static let appOnlyKey = "ClaktoryAppOnly"

  /// The backend the app carries.
  static var helper: URL {
    Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/factoryd")
  }

  /// Whether the app at `path` runs from somewhere it can't stay: a disk
  /// image (/Volumes) or a translocated copy, which macOS makes for an app
  /// opened from where it was downloaded.
  static func needsMove(_ path: String) -> Bool {
    path.hasPrefix("/Volumes/") || path.contains("/AppTranslocation/")
  }

  /// Where the app really is: for a translocated app, the original
  /// macOS made its copy from (SecTranslocateCreateOriginalPathForURL,
  /// which Security has without a header); else where it runs.
  static var originalURL: URL {
    let url = Bundle.main.bundleURL
    guard url.path.contains("/AppTranslocation/"),
      let security = dlopen(
        "/System/Library/Frameworks/Security.framework/Security", RTLD_LAZY),
      let symbol = dlsym(security, "SecTranslocateCreateOriginalPathForURL")
    else { return url }
    typealias Original =
      @convention(c) (CFURL, UnsafeMutablePointer<Unmanaged<CFError>?>?) -> Unmanaged<CFURL>?
    let original = unsafeBitCast(symbol, to: Original.self)
    guard let found = original(url as CFURL, nil)?.takeRetainedValue() else { return url }
    return found as URL
  }

  /// The mounted disk image `url` is on (its mount point, such as
  /// /Volumes/Claktory), if it's on one: a volume `hdiutil info` lists.
  /// Never another disk under /Volumes.
  static func diskImage(of url: URL) -> String? {
    let path = url.standardizedFileURL.path
    guard path.hasPrefix("/Volumes/") else { return nil }
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/hdiutil")
    process.arguments = ["info", "-plist"]
    let out = Pipe()
    process.standardOutput = out
    process.standardError = FileHandle.nullDevice
    guard (try? process.run()) != nil else { return nil }
    let data = out.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    guard
      let info = try? PropertyListSerialization.propertyList(from: data, format: nil)
        as? [String: Any],
      let images = info["images"] as? [[String: Any]]
    else { return nil }
    for image in images {
      for entity in image["system-entities"] as? [[String: Any]] ?? [] {
        if let mount = entity["mount-point"] as? String,
          path == mount || path.hasPrefix(mount + "/")
        {
          return mount
        }
      }
    }
    return nil
  }

  /// Runs `factoryd` from Contents/Helpers with `arguments` (which must
  /// start with "install"), off the main thread, and answers its exit code
  /// and output: {"code": Int, "stdout": String, "stderr": String}, or a
  /// FlutterError when it can't start.
  static func install(_ arguments: [String], _ result: @escaping FlutterResult) {
    guard arguments.first == "install" else {
      return result(FlutterError(code: "arguments", message: "only install", details: nil))
    }
    runFactoryd(helper, arguments, result)
  }

  /// The installed backend, which any Claktory on this Mac has, from the
  /// online installer too.
  static var installed: URL {
    FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent(".factory/bin/factoryd")
  }

  /// Runs `factoryd uninstall` with `arguments` (which must start with
  /// "uninstall"): the one the app carries, which matches it, else the
  /// installed backend's. It's the app's child, so it carries on as the
  /// backend stops.
  static func uninstall(_ arguments: [String], _ result: @escaping FlutterResult) {
    guard arguments.first == "uninstall" else {
      return result(FlutterError(code: "arguments", message: "only uninstall", details: nil))
    }
    let factoryd =
      FileManager.default.isExecutableFile(atPath: helper.path) ? helper : installed
    runFactoryd(factoryd, arguments, result)
  }

  /// Puts this app in the Trash and quits, once Claktory is removed. With
  /// `forget`, what the app keeps on this Mac goes too: its user defaults
  /// (the window's frame, drafts) and its connections in the Keychain.
  static func removeApp(forget: Bool, _ result: @escaping FlutterResult) {
    if forget, let bundleID = Bundle.main.bundleIdentifier {
      try? KeychainItem(service: "Claktory connections", account: bundleID, label: "").delete()
      UserDefaults.standard.removePersistentDomain(forName: bundleID)
      UserDefaults.standard.synchronize()
    }
    // From a disk image there's nothing to put away: it just quits.
    try? FileManager.default.trashItem(at: originalURL, resultingItemURL: nil)
    result(nil)
    // At once, so nothing writes the defaults back on the way out.
    DispatchQueue.main.async { exit(0) }
  }

  /// Runs `factoryd` at `executable` with `arguments`, off the main thread,
  /// and answers its exit code and output: {"code": Int, "stdout": String,
  /// "stderr": String}, or a FlutterError when it can't start.
  private static func runFactoryd(
    _ executable: URL, _ arguments: [String], _ result: @escaping FlutterResult
  ) {
    guard FileManager.default.isExecutableFile(atPath: executable.path) else {
      return result(FlutterError(code: "missing", message: executable.path, details: nil))
    }
    DispatchQueue.global(qos: .userInitiated).async {
      let process = Process()
      process.executableURL = executable
      process.arguments = arguments
      let out = Pipe()
      let err = Pipe()
      process.standardOutput = out
      process.standardError = err
      process.standardInput = FileHandle.nullDevice
      do {
        try process.run()
      } catch {
        DispatchQueue.main.async {
          result(FlutterError(code: "start", message: error.localizedDescription, details: nil))
        }
        return
      }
      // Read both before waiting, so a full pipe can't stall it.
      var stdout = Data()
      var stderr = Data()
      let group = DispatchGroup()
      group.enter()
      DispatchQueue.global().async {
        stderr = err.fileHandleForReading.readDataToEndOfFile()
        group.leave()
      }
      stdout = out.fileHandleForReading.readDataToEndOfFile()
      group.wait()
      process.waitUntilExit()
      let answer: [String: Any] = [
        "code": Int(process.terminationStatus),
        "stdout": String(decoding: stdout, as: UTF8.self),
        "stderr": String(decoding: stderr, as: UTF8.self),
      ]
      DispatchQueue.main.async { result(answer) }
    }
  }

  /// Copies the app into /Applications (or ~/Applications when the person
  /// can't write there) as Claktory.app, putting one already there in the
  /// Trash, and opens the copy once this one has quit, then ejects the disk
  /// image it came from. Answers an error's words when it can't, and quits
  /// when it can. An app that's there already, only translocated, just
  /// loses the quarantine mark and opens again.
  static func moveToApplications(_ result: @escaping FlutterResult) {
    let fileManager = FileManager.default
    let source = Bundle.main.bundleURL
    let original = originalURL
    let system = URL(fileURLWithPath: "/Applications", isDirectory: true)
    let folder: URL
    if fileManager.isWritableFile(atPath: system.path) {
      folder = system
    } else {
      folder = fileManager.homeDirectoryForCurrentUser
        .appendingPathComponent("Applications", isDirectory: true)
      try? fileManager.createDirectory(at: folder, withIntermediateDirectories: true)
    }
    var destination = folder.appendingPathComponent("Claktory.app", isDirectory: true)
    let inPlace =
      original.standardizedFileURL.deletingLastPathComponent().path
      == system.path
      || original.standardizedFileURL.deletingLastPathComponent().path
        == fileManager.homeDirectoryForCurrentUser
        .appendingPathComponent("Applications").standardizedFileURL.path
    if inPlace { destination = original }
    DispatchQueue.global(qos: .userInitiated).async {
      let image = diskImage(of: original)
      do {
        if !inPlace {
          if fileManager.fileExists(atPath: destination.path) {
            try fileManager.trashItem(at: destination, resultingItemURL: nil)
          }
          // ditto keeps the signature, extended attributes and symlinks as
          // they are.
          try run("/usr/bin/ditto", [source.path, destination.path])
        }
        // Moved by the person, as far as macOS is concerned: without the
        // quarantine mark, the copy opens where it is rather than
        // translocated again.
        try? run("/usr/bin/xattr", ["-d", "-r", "com.apple.quarantine", destination.path])
      } catch {
        DispatchQueue.main.async { result(error.localizedDescription) }
        return
      }
      DispatchQueue.main.async {
        // Opens the copy once this process is gone, so macOS doesn't just
        // bring this one forward; then ejects the disk image, now that
        // nothing runs from it.
        let pid = ProcessInfo.processInfo.processIdentifier
        let script = "while /bin/kill -0 \(pid) 2>/dev/null; do /bin/sleep 0.1; done; "
          + "/usr/bin/open \"$1\"; "
          + "if [ -n \"$2\" ]; then /usr/bin/hdiutil detach \"$2\" -quiet "
          + "|| { /bin/sleep 2; /usr/bin/hdiutil detach \"$2\" -quiet; }; fi"
        let relaunch = Process()
        relaunch.executableURL = URL(fileURLWithPath: "/bin/sh")
        relaunch.arguments = ["-c", script, "sh", destination.path, image ?? ""]
        do {
          try relaunch.run()
        } catch {
          result(error.localizedDescription)
          return
        }
        result(nil)
        NSApp.terminate(nil)
      }
    }
  }

  /// Runs `path` with `arguments` and waits; throws its error output when
  /// it fails.
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
      throw NSError(
        domain: "Setup", code: Int(process.terminationStatus),
        userInfo: [
          NSLocalizedDescriptionKey: String(decoding: message, as: UTF8.self)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        ])
    }
  }
}

/// This Mac's Claktory, stopped and started from the app (friend bug #27).
/// factory/engine:
/// - the app asks the window's Dart "quitting" before it quits (⌘Q, or the
///   last window closed), and quits once it's answered, or after a few
///   seconds: Dart stops the engine first when the person chose that;
/// - Dart sets the app menu's Stop Claktory or Start Claktory item with
///   "menu" {title, enabled}, or hides it with nil; choosing it sends
///   "menuChosen", answered with why it was refused, if it was.
enum Engine {
  private static var channel: FlutterMethodChannel?
  private static var item: NSMenuItem?
  private static let target = MenuTarget()
  private static var quitting = false

  /// How long the app waits for Dart before it quits anyway.
  static let quitWithin: TimeInterval = 5

  static func register(with messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: "factory/engine", binaryMessenger: messenger)
    channel.setMethodCallHandler { call, result in
      switch call.method {
      case "menu":
        setItem(call.arguments as? [String: Any])
        result(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
    self.channel = channel
  }

  /// AppDelegate's applicationShouldTerminate.
  static func shouldTerminate(_ app: NSApplication) -> NSApplication.TerminateReply {
    guard let channel, !quitting else { return .terminateNow }
    quitting = true
    var replied = false
    let reply = {
      guard !replied else { return }
      replied = true
      app.reply(toApplicationShouldTerminate: true)
    }
    channel.invokeMethod("quitting", arguments: nil) { _ in
      DispatchQueue.main.async(execute: reply)
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + quitWithin, execute: reply)
    return .terminateLater
  }

  /// The item, in the app menu under About: made the first time it's set.
  private static func setItem(_ settings: [String: Any]?) {
    guard let title = settings?["title"] as? String else {
      item?.isHidden = true
      return
    }
    if item == nil, let menu = NSApp.mainMenu?.items.first?.submenu {
      let made = NSMenuItem(title: title, action: #selector(MenuTarget.chosen(_:)), keyEquivalent: "")
      made.target = target
      // After About and its separator, with one of its own.
      let at = min(2, menu.items.count)
      menu.insertItem(made, at: at)
      menu.insertItem(.separator(), at: at + 1)
      item = made
    }
    item?.title = title
    item?.isHidden = false
    item?.isEnabled = settings?["enabled"] as? Bool ?? true
  }

  final class MenuTarget: NSObject, NSMenuItemValidation {
    @objc func chosen(_ sender: Any?) {
      // Refused (finishing an update, say): its words, in an alert.
      Engine.channel?.invokeMethod("menuChosen", arguments: nil) { answer in
        guard let words = answer as? String, !words.isEmpty else { return }
        DispatchQueue.main.async {
          let alert = NSAlert()
          alert.messageText = "Claktory couldn't stop"
          alert.informativeText = words
          alert.runModal()
        }
      }
    }

    func validateMenuItem(_ menuItem: NSMenuItem) -> Bool {
      Engine.item?.isEnabled ?? true
    }
  }
}
