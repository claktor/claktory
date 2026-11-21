import Darwin
import Foundation

#if canImport(FlutterMacOS)
  import FlutterMacOS
#endif

/// Connecting this Mac to a Claktory from a shell, for an agent setting
/// Claktory up on a server and this Mac at once (#245), without anyone
/// pasting a code or reading digits off the screen.
///
/// The agent writes the pairing code to `connect-request.json` in this
/// app's folder in Application Support (`{"code": "factory://…"}`, mode
/// 0600) and runs `open -g -b <bundle id>`. The app takes it, connects (the
/// Dart side, app/lib/setup_link.dart: only while it has no Claktory saved
/// yet) and says how it goes in `connect-status.json` beside it: connecting,
/// waiting (with the device's id, its four digits and its key for
/// `factoryd allow` on the server), connected, or failed with why.
///
/// Only a program running on this Mac as this person can write that file:
/// a link (claktory://add?code=…), which any web page can open, still asks
/// first. The file must be a regular file of this user's, not a link,
/// readable by no one else and less than ten minutes old; it's removed as
/// soon as it's read, whatever it says.
enum SetupLink {
  static let requestName = "connect-request.json"
  static let statusName = "connect-status.json"
  /// How old a request may be.
  static let maxAge: TimeInterval = 10 * 60
  /// More than any pairing code needs.
  static let maxSize = 16 * 1024

  #if canImport(FlutterMacOS)
    private static var channel: FlutterMethodChannel?
  #endif
  private static var waiting: [String] = []
  private static var listening = false
  private static var watcher: DispatchSourceFileSystemObject?

  /// This app's folder in Application Support, made if it isn't there.
  static func folder() -> URL? {
    guard
      let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)
        .first
    else { return nil }
    let folder = support.appendingPathComponent(
      Bundle.main.bundleIdentifier ?? "dev.factory.app", isDirectory: true)
    try? FileManager.default.createDirectory(
      at: folder, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
    return folder
  }

  #if canImport(FlutterMacOS)
  static func register(with messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: "factory/setup-link", binaryMessenger: messenger)
    channel.setMethodCallHandler { call, result in
      switch call.method {
      case "listen":
        listening = true
        result(waiting)
        waiting = []
      case "status":
        guard let folder = folder(), let status = call.arguments as? [String: Any] else {
          return result(nil)
        }
        writeStatus(status, in: folder)
        result(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
    Self.channel = channel
    guard let folder = folder() else { return }
    take(in: folder)
    watch(folder)
  }
  #endif

  /// Takes a request waiting in [folder], if there's one.
  static func take(in folder: URL) {
    let request = folder.appendingPathComponent(requestName)
    switch read(request) {
    case .none:
      return
    case .refused(let reason):
      writeStatus(["state": "failed", "reason": reason], in: folder)
    case .code(let code):
      #if canImport(FlutterMacOS)
        if listening, let channel {
          channel.invokeMethod("request", arguments: code)
          return
        }
      #endif
      waiting.append(code)
    }
  }

  enum Request: Equatable {
    /// No request there.
    case none
    /// One that can't be taken, and why, for the status.
    case refused(String)
    /// The pairing code it asks to connect with.
    case code(String)
  }

  /// The request at [url], removed once read whatever it says. It's
  /// opened without following a link, and checked on what was opened, so
  /// nothing can be swapped in between the checks and the reading.
  static func read(_ url: URL, now: Date = Date()) -> Request {
    let fd = open(url.path, O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)
    if fd < 0 {
      if errno == ENOENT { return .none }
      // A link where the file should be (ELOOP), or one that can't be read.
      unlink(url.path)
      return .refused(errno == ELOOP ? "not_a_file" : "unreadable")
    }
    defer { close(fd) }
    // Gone at once: a request is used once, or not at all.
    unlink(url.path)
    var info = stat()
    guard fstat(fd, &info) == 0 else { return .refused("unreadable") }
    guard info.st_mode & S_IFMT == S_IFREG else { return .refused("not_a_file") }
    guard info.st_uid == getuid() else { return .refused("not_yours") }
    guard info.st_mode & 0o077 == 0 else { return .refused("readable_by_others") }
    let modified = Date(
      timeIntervalSince1970: TimeInterval(info.st_mtimespec.tv_sec)
        + TimeInterval(info.st_mtimespec.tv_nsec) / 1e9)
    // Older than a request lasts, or dated ahead (by more than a clock's
    // slip), which would never grow old.
    let age = now.timeIntervalSince(modified)
    guard age <= maxAge, age >= -60 else { return .refused("too_old") }
    guard info.st_size <= maxSize else { return .refused("too_big") }
    let size = Int(info.st_size)
    var bytes = [UInt8](repeating: 0, count: size)
    var got = 0
    while got < size {
      let n = bytes.withUnsafeMutableBytes { buffer in
        Darwin.read(fd, buffer.baseAddress! + got, size - got)
      }
      if n <= 0 { break }
      got += n
    }
    guard got == size,
      let object = try? JSONSerialization.jsonObject(with: Data(bytes)) as? [String: Any],
      let code = object["code"] as? String,
      !code.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    else { return .refused("no_code") }
    return .code(code.trimmingCharacters(in: .whitespacesAndNewlines))
  }

  /// [status] as `connect-status.json` in [folder], readable only by this
  /// user, replaced whole so a reader never sees half of it.
  static func writeStatus(_ status: [String: Any], in folder: URL) {
    var status = status
    status["at_ms"] = Int(Date().timeIntervalSince1970 * 1000)
    guard
      let data = try? JSONSerialization.data(
        withJSONObject: status, options: [.sortedKeys])
    else { return }
    let target = folder.appendingPathComponent(statusName)
    let temporary = folder.appendingPathComponent(".\(statusName).\(getpid())")
    unlink(temporary.path)
    let fd = open(temporary.path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0o600)
    guard fd >= 0 else { return }
    let wrote = data.withUnsafeBytes { write(fd, $0.baseAddress, data.count) }
    close(fd)
    guard wrote == data.count else {
      unlink(temporary.path)
      return
    }
    if rename(temporary.path, target.path) != 0 { unlink(temporary.path) }
  }

  /// Takes a request as soon as one is written in [folder].
  static func watch(_ folder: URL) {
    let fd = open(folder.path, O_EVTONLY)
    guard fd >= 0 else { return }
    let source = DispatchSource.makeFileSystemObjectSource(
      fileDescriptor: fd, eventMask: [.write, .rename, .link], queue: .main)
    // A moment after the last change: the agent's `mv` into place, or a
    // write still going on.
    var pending: DispatchWorkItem?
    source.setEventHandler {
      pending?.cancel()
      let next = DispatchWorkItem { take(in: folder) }
      pending = next
      DispatchQueue.main.asyncAfter(deadline: .now() + 0.5, execute: next)
    }
    source.setCancelHandler { close(fd) }
    source.resume()
    watcher = source
  }
}
