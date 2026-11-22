// The Mac app's end of browser control (B4a): where Chrome's
// native-messaging helper (claktory-browser-host, started by Chrome for our
// extension) connects, so the app can relay a session's commands and
// results. Foundation and Security only, so it's checked without the app
// (RunnerTests/BrowserBrokerCheck.swift); BrowserBridge.swift puts it on
// the factory/browser channel.
//
// - A Unix socket in a private folder (0700, ours, not a link; the socket
//   0600). No TCP listener, no debug port.
// - Every peer is checked from the socket's audit token (LOCAL_PEERTOKEN →
//   SecCodeCopyGuestWithAttributes → SecCodeCheckValidity) against the
//   helper's requirement, never from a PID or a path. A peer that fails, or
//   is gone before it's checked, is closed: nothing reaches the app.
// - Frames: a little-endian u32 length, then one JSON object, 256 KiB at
//   most both ways, checked before anything is allocated for it. Anything
//   else closes the connection.
// - The helper's first frame says who it is ({"type":"helper","v":1,…});
//   one that doesn't within 5 s, or speaks another version, is closed.
// - A connected helper grants nothing: it gets an ephemeral id, and only the
//   person's local choice (the app's UI) binds it to a session. One that
//   isn't bound within 5 minutes is closed.
import Foundation
import Security

/// What the broker tells the app.
enum BrowserBrokerEvent: Equatable {
  /// A helper passed the signature check and said who it is. [code] is
  /// the pairing digits the app sent it, for its popup: the app shows the
  /// same beside it, so the person can tell which tab is which.
  case connected(id: Int, version: String, caller: String, code: String)
  /// One JSON object from it, as it came.
  case message(id: Int, json: Data)
  /// It's gone (closed, refused a frame, timed out, or closed by the app):
  /// a [BrokerCode] (the app words it) and a technical detail.
  case gone(id: Int, code: String, detail: String)
  /// A peer that never became a helper: why, without anything it sent.
  case refused(code: String, detail: String)
}

/// Who may connect: a code requirement, as text, for the log too.
struct HelperRequirement {
  let text: String

  /// The release helper, signed by the app's own team. An app without a
  /// team trusts no helper, with one exception that needs both a Debug
  /// build (compiled in only there: no release, --dev or --adhoc build has
  /// it) and an app signed ad hoc: the Dev helper inside its own bundle.
  static func forThisApp(bundle: Bundle = .main) -> Result<HelperRequirement, BrokerError> {
    let me = CodeIdentity.ofSelf()
    if let team = me?.team {
      return .success(
        HelperRequirement(
          text: "anchor apple generic and certificate leaf[subject.OU] = \"\(team)\" "
            + "and identifier \"\(BrowserBroker.helperIdentifier)\""))
    }
    #if DEBUG
      return devHelper(bundle: bundle)
    #else
      return .failure(BrokerError(BrokerCode.unsigned, "no team ID in this app's signature"))
    #endif
  }

  #if DEBUG
    private static func devHelper(bundle: Bundle) -> Result<HelperRequirement, BrokerError> {
      let helper = bundle.bundleURL.appendingPathComponent("Contents/Helpers/\(BrowserBroker.helperName)")
      guard let cdhash = CodeIdentity.ofFile(helper)?.cdhash else {
        return .failure(BrokerError(BrokerCode.devNoHelper, "no signed helper in Contents/Helpers"))
      }
      return .success(
        HelperRequirement(
          text: "identifier \"\(BrowserBroker.devHelperIdentifier)\" and cdhash H\"\(cdhash)\""))
    }
  #endif
}

/// Why something failed, for the app to word (its words live in Dart,
/// keyed by [code]), and a technical detail to show faintly.
struct BrokerError: Error, Equatable {
  let code: String
  let detail: String
  init(_ code: String, _ detail: String = "") {
    self.code = code
    self.detail = detail
  }
}

/// The codes the broker says. Start: unsigned, devNoHelper, pathTooLong,
/// folderUnsafe, cantListen. A peer turned away: notHelper. A helper gone:
/// closed, unexpected, version, notChosen, stopped, or the app's own
/// reason for ending it.
enum BrokerCode {
  static let unsigned = "unsigned"
  static let devNoHelper = "dev_no_helper"
  static let pathTooLong = "path_too_long"
  static let folderUnsafe = "folder_unsafe"
  static let cantListen = "cant_listen"
  static let notHelper = "not_helper"
  static let closed = "closed"
  static let unexpected = "unexpected"
  static let version = "version"
  static let notChosen = "not_chosen"
  static let stopped = "stopped"
  /// The person's Stop: the app's, or the review panel's.
  static let stop = "stop"
}

/// A process's or a file's signature, as far as the broker needs it.
struct CodeIdentity {
  let identifier: String
  let team: String?
  let cdhash: String

  static func ofSelf() -> CodeIdentity? {
    var code: SecCode?
    guard SecCodeCopySelf([], &code) == errSecSuccess, let code else { return nil }
    var staticCode: SecStaticCode?
    guard SecCodeCopyStaticCode(code, [], &staticCode) == errSecSuccess, let staticCode else {
      return nil
    }
    return of(staticCode)
  }

  static func ofFile(_ url: URL) -> CodeIdentity? {
    var staticCode: SecStaticCode?
    guard SecStaticCodeCreateWithPath(url as CFURL, [], &staticCode) == errSecSuccess,
      let staticCode
    else { return nil }
    return of(staticCode)
  }

  static func of(_ code: SecStaticCode) -> CodeIdentity? {
    var info: CFDictionary?
    guard
      SecCodeCopySigningInformation(code, SecCSFlags(rawValue: kSecCSSigningInformation), &info)
        == errSecSuccess,
      let dict = info as? [String: Any],
      let identifier = dict[kSecCodeInfoIdentifier as String] as? String,
      let unique = dict[kSecCodeInfoUnique as String] as? Data
    else { return nil }
    return CodeIdentity(
      identifier: identifier,
      team: dict[kSecCodeInfoTeamIdentifier as String] as? String,
      cdhash: unique.map { String(format: "%02x", $0) }.joined())
  }
}

final class BrowserBroker {
  static let helperName = "claktory-browser-host"
  static let helperIdentifier = "com.claktory.browser-host"
  static let devHelperIdentifier = "com.claktory.browser-host.dev"
  static let protocolVersion = 1
  static let frameLimit = 256 * 1024

  /// How long a new helper has to say who it is, and to be bound.
  var helloWithin: TimeInterval = 5
  var boundWithin: TimeInterval = 5 * 60

  private let folder: URL
  private let requirement: HelperRequirement
  private let events: (BrowserBrokerEvent) -> Void
  /// Everything below, one at a time.
  private let queue = DispatchQueue(label: "factory.browser.broker")
  private var server: Int32 = -1
  private var helpers: [Int: Helper] = [:]
  private var nextId = 1

  private final class Helper {
    let fd: Int32
    var said = false
    var bound = false
    /// The extension's review waiting for the person: the command it's
    /// for, its hash and when it lapses (epoch ms, this Mac's clock).
    var review: (command: Int, hash: String, expiresMs: Int64)?
    init(fd: Int32) { self.fd = fd }
  }

  /// The app's own version (its FACTORY_VERSION, such as v0.1.17): a
  /// helper saying another is from another copy of the app, and is closed.
  /// Set before [start]; empty, or either side's not a release's, checks
  /// nothing.
  var appVersion = ""

  /// This Mac's clock in epoch ms, as the extension's expires_ms counts.
  var nowMs: () -> Int64 = { Int64(Date().timeIntervalSince1970 * 1000) }

  /// [events] come on the broker's own queue: act on one from elsewhere
  /// (the platform channel does), never by calling the broker back inside
  /// it, which would wait on that same queue forever.
  init(folder: URL, requirement: HelperRequirement, events: @escaping (BrowserBrokerEvent) -> Void) {
    self.folder = folder
    self.requirement = requirement
    self.events = events
  }

  /// The release or Dev folder in Application Support.
  static func folder(dev: Bool) -> URL {
    FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent("Library/Application Support")
      .appendingPathComponent(dev ? "Claktory Dev" : "Claktory")
      .appendingPathComponent("browser")
  }

  var socketPath: String { folder.appendingPathComponent("broker.sock").path }

  // MARK: Listening

  /// Listens; the socket's path. Fails, saying why, rather than listening
  /// anywhere less private.
  func start() -> Result<String, BrokerError> {
    queue.sync { startLocked() }
  }

  private func startLocked() -> Result<String, BrokerError> {
    if server >= 0 { return .success(socketPath) }
    var req: SecRequirement?
    guard SecRequirementCreateWithString(requirement.text as CFString, [], &req) == errSecSuccess,
      let req
    else { return .failure(BrokerError(BrokerCode.cantListen, "the helper's requirement doesn't parse")) }
    secRequirement = req
    if let why = BrowserBroker.makePrivate(folder) { return .failure(why) }
    let path = socketPath
    let bytes = Array(path.utf8)
    var addr = sockaddr_un()
    // Never truncated: a path that doesn't fit isn't used.
    guard bytes.count < MemoryLayout.size(ofValue: addr.sun_path) else {
      return .failure(
        BrokerError(BrokerCode.pathTooLong, "the socket path is \(bytes.count) bytes; macOS allows 103"))
    }
    addr.sun_family = sa_family_t(AF_UNIX)
    withUnsafeMutableBytes(of: &addr.sun_path) { $0.copyBytes(from: bytes) }
    unlink(path)
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { return .failure(BrokerError(BrokerCode.cantListen, BrowserBroker.errorText())) }
    // Made 0600 from the start: no moment when another user could connect.
    let old = umask(0o177)
    let bound = withUnsafePointer(to: &addr) {
      $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
        bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
      }
    }
    umask(old)
    guard bound == 0, listen(fd, 8) == 0 else {
      let why = BrowserBroker.errorText()
      close(fd)
      return .failure(BrokerError(BrokerCode.cantListen, why))
    }
    server = fd
    Thread.detachNewThread { [weak self] in self?.accepting(fd) }
    return .success(path)
  }

  private var secRequirement: SecRequirement?

  /// Whether a helper's version and the app's are two different releases'.
  /// The helper inside the app is replaced with it, so a different one is
  /// another copy's, reached through a manifest left pointing there. Only
  /// releases' versions (v and a digit) are compared: a plain build has
  /// none to go by.
  static func versionsDiffer(helper: String, app: String) -> Bool {
    func release(_ version: String) -> Bool {
      version.count > 1 && version.hasPrefix("v") && version.dropFirst().first!.isNumber
    }
    return release(helper) && release(app) && helper != app
  }

  /// Stops listening and closes every helper.
  func stop() {
    queue.sync {
      if server >= 0 {
        shutdown(server, SHUT_RDWR)
        close(server)
      }
      server = -1
      for id in Array(helpers.keys) { closeLocked(id, BrokerCode.stopped) }
    }
  }

  /// The folder, made 0700 if it isn't there; nil when it's private and
  /// ours, else why not.
  static func makePrivate(_ folder: URL) -> BrokerError? {
    let path = folder.path
    try? FileManager.default.createDirectory(
      at: folder.deletingLastPathComponent(), withIntermediateDirectories: true)
    mkdir(path, 0o700)
    var st = stat()
    guard lstat(path, &st) == 0 else { return BrokerError(BrokerCode.folderUnsafe, "\(path) can't be made") }
    guard (st.st_mode & S_IFMT) == S_IFDIR else {
      return BrokerError(BrokerCode.folderUnsafe, "\(path) is a link or not a folder")
    }
    guard st.st_uid == getuid() else { return BrokerError(BrokerCode.folderUnsafe, "\(path) belongs to another user") }
    guard st.st_mode & 0o077 == 0 else {
      return BrokerError(BrokerCode.folderUnsafe, "\(path) is open to other users")
    }
    return nil
  }

  private func accepting(_ server: Int32) {
    while true {
      let fd = accept(server, nil, nil)
      if fd < 0 {
        if errno == EINTR || errno == ECONNABORTED { continue }
        return
      }
      var on: Int32 = 1
      setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
      switch check(fd) {
      case .failure(let why):
        close(fd)
        queue.async { self.events(.refused(code: why.code, detail: why.detail)) }
      case .success:
        let id: Int = queue.sync {
          let id = nextId
          nextId += 1
          helpers[id] = Helper(fd: fd)
          return id
        }
        Thread.detachNewThread { [weak self] in self?.reading(id, fd) }
        queue.asyncAfter(deadline: .now() + helloWithin) { [weak self] in
          guard let self, let helper = self.helpers[id], !helper.said else { return }
          self.closeLocked(id, BrokerCode.unexpected, "no hello within \(Int(self.helloWithin)) s")
        }
        queue.asyncAfter(deadline: .now() + boundWithin) { [weak self] in
          guard let self, let helper = self.helpers[id], !helper.bound else { return }
          self.closeLocked(id, BrokerCode.notChosen, "not chosen within \(Int(self.boundWithin)) s")
        }
      }
    }
  }

  // MARK: The peer's signature

  /// The process on the other end, by its audit token, against the
  /// requirement. Fails closed: no token, no code (gone already), no match.
  private func check(_ fd: Int32) -> Result<Void, BrokerError> {
    var token = audit_token_t()
    var length = socklen_t(MemoryLayout<audit_token_t>.size)
    guard getsockopt(fd, SOL_LOCAL, LOCAL_PEERTOKEN, &token, &length) == 0 else {
      return .failure(BrokerError(BrokerCode.notHelper, "no audit token"))
    }
    let tokenData = withUnsafeBytes(of: &token) { Data($0) }
    var code: SecCode?
    guard
      SecCodeCopyGuestWithAttributes(
        nil, [kSecGuestAttributeAudit: tokenData] as CFDictionary, [], &code) == errSecSuccess,
      let code
    else { return .failure(BrokerError(BrokerCode.notHelper, "gone before it was checked")) }
    guard let requirement = queue.sync(execute: { secRequirement }) else {
      return .failure(BrokerError(BrokerCode.notHelper, "not listening"))
    }
    let status = SecCodeCheckValidity(code, [], requirement)
    guard status == errSecSuccess else {
      var staticCode: SecStaticCode?
      SecCodeCopyStaticCode(code, [], &staticCode)
      let who = staticCode.flatMap { CodeIdentity.of($0)?.identifier } ?? "an unsigned process"
      return .failure(BrokerError(BrokerCode.notHelper, "\(who) (\(status))"))
    }
    return .success(())
  }

  // MARK: Frames

  private func reading(_ id: Int, _ fd: Int32) {
    while true {
      guard let head = BrowserBroker.readExactly(fd, 4) else {
        return queue.async { self.closeLocked(id, BrokerCode.closed) }
      }
      let length = Int(UInt32(littleEndian: head.withUnsafeBytes { $0.loadUnaligned(as: UInt32.self) }))
      // Checked before anything is allocated for it.
      guard length > 0, length <= BrowserBroker.frameLimit else {
        return queue.async { self.closeLocked(id, BrokerCode.unexpected, "a frame of \(length) bytes") }
      }
      guard let frame = BrowserBroker.readExactly(fd, length) else {
        return queue.async { self.closeLocked(id, BrokerCode.unexpected, "closed mid-frame") }
      }
      guard let object = try? JSONSerialization.jsonObject(with: frame) as? [String: Any] else {
        return queue.async { self.closeLocked(id, BrokerCode.unexpected, "a frame that isn't a JSON object") }
      }
      queue.async { self.received(id, object, frame) }
    }
  }

  private func received(_ id: Int, _ object: [String: Any], _ frame: Data) {
    guard let helper = helpers[id] else { return }
    if !helper.said {
      guard object["type"] as? String == "helper" else {
        return closeLocked(id, BrokerCode.unexpected, "its first frame isn't hello")
      }
      guard object["v"] as? Int == BrowserBroker.protocolVersion else {
        return closeLocked(id, BrokerCode.version, "version \(object["v"] ?? "none")")
      }
      let helperVersion = object["version"] as? String ?? ""
      if BrowserBroker.versionsDiffer(helper: helperVersion, app: appVersion) {
        return closeLocked(id, BrokerCode.version, "helper \(helperVersion), app \(appVersion)")
      }
      helper.said = true
      // The app's own digits, never the helper's: the app is where the
      // person decides.
      let code = BrowserBroker.pairingCode()
      guard writeFrame(helper.fd, ["type": "pair", "code": code]) else {
        return closeLocked(id, BrokerCode.closed)
      }
      events(
        .connected(
          id: id, version: object["version"] as? String ?? "",
          caller: object["caller"] as? String ?? "", code: code))
      return
    }
    // A review waits for the person's decision ([decide]) until the
    // command's result, a pause or its end; a new one replaces it.
    switch object["type"] as? String {
    case "review":
      if let command = object["id"] as? Int, let hash = object["hash"] as? String,
        let expires = (object["expires_ms"] as? NSNumber)?.int64Value
      {
        helper.review = (command, hash, expires)
      } else {
        helper.review = nil
      }
    case "result", "paused", "ended":
      helper.review = nil
    default:
      break
    }
    events(.message(id: id, json: frame))
  }

  /// Sends one JSON object to a helper; false when it isn't one, is too
  /// big, is a decision (only [decide] sends those), or the helper is gone.
  @discardableResult
  func send(_ id: Int, json: Data) -> Bool {
    guard json.count <= BrowserBroker.frameLimit,
      let object = (try? JSONSerialization.jsonObject(with: json)) as? [String: Any],
      object["type"] as? String != "decision"
    else { return false }
    return queue.sync {
      guard let helper = helpers[id], helper.said else { return false }
      var length = UInt32(json.count).littleEndian
      let head = withUnsafeBytes(of: &length) { Data($0) }
      // As the app gave it, byte for byte.
      return BrowserBroker.writeAll(helper.fd, head) && BrowserBroker.writeAll(helper.fd, json)
    }
  }

  /// The person's own decision, from the review panel on this Mac and
  /// nothing else (only [ReviewDesk.press] makes a [ReviewClick]): sent
  /// only for the review its helper sent (its command and hash), before it
  /// lapses, once. False, and nothing sent, otherwise.
  func decide(_ click: ReviewClick) -> Bool {
    queue.sync {
      guard let helper = helpers[click.helper], helper.bound, let review = helper.review,
        review.command == click.command, review.hash == click.hash, nowMs() < review.expiresMs
      else { return false }
      helper.review = nil
      return writeFrame(
        helper.fd,
        ["type": "decision", "id": click.command, "hash": click.hash, "allow": click.allow])
    }
  }

  /// The person chose this helper: it's no longer closed for waiting.
  func choose(_ id: Int) -> Bool {
    queue.sync {
      guard let helper = helpers[id], helper.said else { return false }
      helper.bound = true
      return true
    }
  }

  /// Local Stop, or done with it: says stop, then closes.
  func end(_ id: Int, code: String) {
    queue.sync {
      if let helper = helpers[id], helper.said {
        _ = writeFrame(helper.fd, ["type": "stop"])
      }
      closeLocked(id, code)
    }
  }

  private func closeLocked(_ id: Int, _ code: String, _ detail: String = "") {
    guard let helper = helpers.removeValue(forKey: id) else { return }
    shutdown(helper.fd, SHUT_RDWR)
    close(helper.fd)
    events(.gone(id: id, code: code, detail: detail))
  }

  // MARK: Plumbing

  /// Six digits from the system's random source.
  static func pairingCode() -> String {
    var value: UInt32 = 0
    _ = withUnsafeMutableBytes(of: &value) { SecRandomCopyBytes(kSecRandomDefault, 4, $0.baseAddress!) }
    return String(format: "%06u", value % 1_000_000)
  }

  private func writeFrame(_ fd: Int32, _ object: [String: Any]) -> Bool {
    guard let json = try? JSONSerialization.data(withJSONObject: object) else { return false }
    var length = UInt32(json.count).littleEndian
    let head = withUnsafeBytes(of: &length) { Data($0) }
    return BrowserBroker.writeAll(fd, head) && BrowserBroker.writeAll(fd, json)
  }

  static func readExactly(_ fd: Int32, _ count: Int) -> Data? {
    var data = Data(count: count)
    var got = 0
    while got < count {
      let n = data.withUnsafeMutableBytes { read(fd, $0.baseAddress! + got, count - got) }
      if n < 0 && errno == EINTR { continue }
      if n <= 0 { return nil }
      got += n
    }
    return data
  }

  static func writeAll(_ fd: Int32, _ data: Data) -> Bool {
    var sent = 0
    while sent < data.count {
      let n = data.withUnsafeBytes { write(fd, $0.baseAddress! + sent, data.count - sent) }
      if n < 0 && errno == EINTR { continue }
      if n <= 0 { return false }
      sent += n
    }
    return true
  }

  static func errorText() -> String { String(cString: strerror(errno)) }
}

/// Chrome's manifest that lets it start the helper: written and removed only
/// by the helper itself (--register, --unregister: whole files, never
/// through a link, never over another program's), and only after the person
/// chose browser control in the app. At launch the app only keeps an
/// existing one pointing at its own helper: the file being there is the
/// record of that choice, so nothing else is kept on the Mac for it.
enum ChromeManifest {
  /// The native messaging host each flavor's helper registers as.
  static func host(dev: Bool) -> String { dev ? "com.claktory.browser.dev" : "com.claktory.browser" }

  /// Google Chrome's user-level manifest for that host.
  static func file(dev: Bool, home: URL = FileManager.default.homeDirectoryForCurrentUser) -> URL {
    home.appendingPathComponent("Library/Application Support/Google/Chrome/NativeMessagingHosts")
      .appendingPathComponent("\(host(dev: dev)).json")
  }

  /// The path [file] names, when it's a plain file naming [host]; nil when
  /// it isn't there, is a link or anything else, or isn't for [host].
  static func registered(_ file: URL, host: String) -> String? {
    var st = stat()
    guard lstat(file.path, &st) == 0, (st.st_mode & S_IFMT) == S_IFREG,
      let data = FileManager.default.contents(atPath: file.path),
      let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
      object["name"] as? String == host,
      let path = object["path"] as? String
    else { return nil }
    return path
  }

  /// At launch: whether to have [helper] register again, which repairs the
  /// manifest after the app moved or was replaced. Only for a manifest the
  /// person already chose (ours, there) that names another path; never
  /// when there's none.
  static func needsRepair(_ file: URL, host: String, helper: URL) -> Bool {
    guard let path = registered(file, host: host) else { return false }
    return path != realPath(helper)
  }

  /// [url] with every link resolved, as the helper names itself.
  static func realPath(_ url: URL) -> String {
    guard let resolved = realpath(url.path, nil) else { return url.path }
    defer { free(resolved) }
    return String(cString: resolved)
  }
}
