import Cocoa
import FlutterMacOS

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
      case "moveToApplications":
        moveToApplications(result)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
    // Kept for as long as the app runs.
    channels.append(channel)
  }

  private static var channels: [FlutterMethodChannel] = []

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
    guard FileManager.default.isExecutableFile(atPath: helper.path) else {
      return result(FlutterError(code: "missing", message: helper.path, details: nil))
    }
    DispatchQueue.global(qos: .userInitiated).async {
      let process = Process()
      process.executableURL = helper
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
