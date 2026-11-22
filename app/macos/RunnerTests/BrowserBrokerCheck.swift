// Checks BrowserBroker (Runner/BrowserBroker.swift) and the review desk
// (Runner/BrowserReview.swift) without the app: its own socket in a
// temporary folder, and this check itself as the peer (a requirement naming
// another program refuses it; one pinning its own signature lets it in, so
// the frames and timeouts can be checked). Run it from app/macos: it also
// reads Runner/BrowserBridge.swift, to check no channel method decides a
// review.
//
//   cd app/macos && swiftc -parse-as-library -o /tmp/browser-broker-check \
//     Runner/BrowserBroker.swift Runner/BrowserReview.swift \
//     RunnerTests/BrowserBrokerCheck.swift && /tmp/browser-broker-check
import Foundation

/// The events a broker said, waited for.
final class Heard {
  private var events: [BrowserBrokerEvent] = []
  private let lock = NSCondition()

  func add(_ event: BrowserBrokerEvent) {
    lock.lock()
    events.append(event)
    lock.broadcast()
    lock.unlock()
  }

  /// The first event that matches, waiting up to [within].
  func wait(within: TimeInterval = 3, _ matches: (BrowserBrokerEvent) -> Bool) -> BrowserBrokerEvent? {
    let end = Date().addingTimeInterval(within)
    lock.lock()
    defer { lock.unlock() }
    while true {
      if let index = events.firstIndex(where: matches) { return events.remove(at: index) }
      if !lock.wait(until: end) { return nil }
    }
  }

  var all: [BrowserBrokerEvent] {
    lock.lock()
    defer { lock.unlock() }
    return events
  }
}

/// This process's end of a connection to the broker.
final class Peer {
  let fd: Int32

  init?(_ path: String) {
    fd = socket(AF_UNIX, SOCK_STREAM, 0)
    var on: Int32 = 1
    setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
    var addr = sockaddr_un()
    addr.sun_family = sa_family_t(AF_UNIX)
    withUnsafeMutableBytes(of: &addr.sun_path) { $0.copyBytes(from: Array(path.utf8)) }
    let ok = withUnsafePointer(to: &addr) {
      $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
        connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
      }
    }
    if ok != 0 { return nil }
  }

  func raw(_ data: Data) { _ = BrowserBroker.writeAll(fd, data) }

  func frame(_ json: String) { frame(Data(json.utf8)) }

  func frame(_ data: Data) {
    var length = UInt32(data.count).littleEndian
    raw(withUnsafeBytes(of: &length) { Data($0) } + data)
  }

  func hello(v: Int = 1, version: String = "0.1.0") {
    frame(#"{"type":"helper","v":\#(v),"version":"\#(version)","caller":"chrome-extension://abc/"}"#)
  }

  /// The next frame, or nil once the broker closed.
  func read() -> String? {
    guard let head = BrowserBroker.readExactly(fd, 4) else { return nil }
    let length = Int(UInt32(littleEndian: head.withUnsafeBytes { $0.loadUnaligned(as: UInt32.self) }))
    return BrowserBroker.readExactly(fd, length).map { String(decoding: $0, as: UTF8.self) }
  }

  /// The next frame if one comes within [within]; nil if none does (or
  /// the broker closed).
  func read(within: TimeInterval) -> String? {
    var waiting = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
    guard poll(&waiting, 1, Int32(within * 1000)) == 1 else { return nil }
    return read()
  }

  deinit { close(fd) }
}

/// The panels as the desk would have them shown.
final class Shown: ReviewPresenter {
  var open: [Int: ReviewContent] = [:]
  var states: [Int: ReviewState] = [:]
  func show(_ id: Int, _ content: ReviewContent, _ state: ReviewState, words: ReviewWords) {
    open[id] = content
    states[id] = state
  }
  func tick(_ id: Int, secondsLeft: Int) {}
  func close(_ id: Int) {
    open[id] = nil
    states[id] = nil
  }
  func front(_ id: Int) -> Bool { open[id] != nil }
}

@main
enum BrowserBrokerCheck {
  static func main() {
    var names: [String] = []
    var failures = 0
    func check(_ ok: Bool, _ what: String, line: Int = #line) {
      names.append(what)
      if !ok {
        failures += 1
        print("FAIL (line \(line)): \(what)")
      }
    }

    let root = FileManager.default.temporaryDirectory
      .appendingPathComponent("bb-\(UUID().uuidString.prefix(8))")
    defer { try? FileManager.default.removeItem(at: root) }
    let me = CodeIdentity.ofSelf()!
    let mine = HelperRequirement(text: "identifier \"\(me.identifier)\" and cdhash H\"\(me.cdhash)\"")

    // The folder: private and ours, or nothing.
    let fresh = root.appendingPathComponent("fresh")
    check(BrowserBroker.makePrivate(fresh) == nil, "a new folder is made private")
    var st = stat()
    lstat(fresh.path, &st)
    check(st.st_mode & 0o777 == 0o700, "made 0700")
    let open = root.appendingPathComponent("open")
    mkdir(open.path, 0o755)
    chmod(open.path, 0o755)
    check(BrowserBroker.makePrivate(open) != nil, "a folder open to others is refused")
    let link = root.appendingPathComponent("link")
    symlink(fresh.path, link.path)
    check(BrowserBroker.makePrivate(link) != nil, "a link is refused, even to a private folder")
    let long = root.appendingPathComponent(String(repeating: "x", count: 120))
    let tooLong = BrowserBroker(folder: long, requirement: mine) { _ in }
    if case .failure(let why) = tooLong.start() {
      check(why.code == BrokerCode.pathTooLong && why.detail.contains("macOS allows 103"), "a socket path past sun_path is refused, saying why")
    } else {
      check(false, "a socket path past sun_path is refused")
    }

    // Another program's requirement: this process is refused, and nothing
    // it sends reaches the app.
    do {
      let heard = Heard()
      let folder = root.appendingPathComponent("other")
      let broker = BrowserBroker(
        folder: folder,
        requirement: HelperRequirement(text: "identifier \"com.claktory.browser-host\""),
        events: heard.add)
      guard case .success(let path) = broker.start() else { return check(false, "listens") }
      var sock = stat()
      lstat(path, &sock)
      check(sock.st_mode & 0o777 == 0o600, "the socket is 0600")
      let peer = Peer(path)!
      peer.hello()
      let refused = heard.wait { if case .refused = $0 { true } else { false } }
      if case .refused(let code, let detail) = refused {
        check(code == BrokerCode.notHelper && !detail.isEmpty, "a peer failing the requirement is refused")
      } else {
        check(false, "a peer failing the requirement is refused")
      }
      check(peer.read() == nil, "and closed")
      check(!heard.all.contains { if case .connected = $0 { true } else { false } }, "and never connected")
      broker.stop()
    }

    // This process's own signature: let in.
    let heard = Heard()
    let broker = BrowserBroker(folder: root.appendingPathComponent("mine"), requirement: mine, events: heard.add)
    broker.helloWithin = 0.5
    broker.boundWithin = 1.5
    guard case .success(let path) = broker.start() else { return check(false, "listens") }

    func connected(_ peer: Peer) -> Int? {
      peer.hello()
      if case .connected(let id, let version, let caller, let code) = heard.wait({
        if case .connected = $0 { true } else { false }
      }) {
        check(version == "0.1.0" && caller == "chrome-extension://abc/", "it says who it is")
        check(code.count == 6 && code.allSatisfy(\.isNumber), "the app makes six pairing digits")
        let pair = peer.read().flatMap { try? JSONSerialization.jsonObject(with: Data($0.utf8)) as? [String: String] }
        check(pair == ["type": "pair", "code": code], "and sends the helper the same")
        return id
      }
      return nil
    }

    func gone(_ id: Int) -> String? {
      if case .gone(_, let code, let detail) = heard.wait({ if case .gone(id, _, _) = $0 { true } else { false } }) {
        return detail.isEmpty ? code : "\(code): \(detail)"
      }
      return nil
    }

    func goneAny() -> String? {
      if case .gone(_, let code, let detail) = heard.wait({ if case .gone = $0 { true } else { false } }) {
        return detail.isEmpty ? code : "\(code): \(detail)"
      }
      return nil
    }

    // A helper: frames both ways, as they are.
    do {
      let peer = Peer(path)!
      guard let id = connected(peer) else { return check(false, "a peer meeting the requirement connects") }
      check(broker.choose(id), "the person's choice binds it")
      peer.frame(#"{"type":"result","id":7,"result":{"ok":true}}"#)
      if case .message(_, let json) = heard.wait({ if case .message = $0 { true } else { false } }) {
        check(String(decoding: json, as: UTF8.self) == #"{"type":"result","id":7,"result":{"ok":true}}"#, "its frames reach the app as sent")
      } else {
        check(false, "its frames reach the app")
      }
      check(broker.send(id, json: Data(#"{"id":8,"cmd":{"op":"read"}}"#.utf8)), "the app's command is sent")
      check(peer.read() == #"{"id":8,"cmd":{"op":"read"}}"#, "and arrives little-endian framed")
      check(!broker.send(id, json: Data("[1]".utf8)), "a command that isn't a JSON object isn't sent")
      check(!broker.send(id, json: Data(count: BrowserBroker.frameLimit + 1)), "nor one past 256 KiB")
      // Chosen: still there past the time to be chosen.
      Thread.sleep(forTimeInterval: 1.7)
      check(broker.send(id, json: Data(#"{"id":9}"#.utf8)), "a chosen helper stays")
      _ = peer.read()
      // Local Stop: says stop, then closes.
      broker.end(id, code: "stop")
      check(peer.read() == #"{"type":"stop"}"#, "Stop says stop to the helper")
      check(peer.read() == nil, "then closes")
      check(gone(id) == "stop", "and says it's gone, with the app's code")
      check(!broker.send(id, json: Data(#"{"id":10}"#.utf8)), "nothing is sent to it after")
    }

    // A review: decided only by the person's press on its panel, armed,
    // in time, once; never through send.
    do {
      let peer = Peer(path)!
      guard let id = connected(peer) else { return check(false, "connects") }
      check(!broker.send(id, json: Data(#"{"type":"decision","id":5,"hash":"ab","allow":true}"#.utf8)), "send never carries a decision")
      check(peer.read(within: 0.3) == nil, "nothing reaches the helper")
      _ = broker.choose(id)
      var clock = Date().timeIntervalSince1970
      var told: [(Int, Int, Bool)] = []
      var stopped: [Int] = []
      let shown = Shown()
      let desk = ReviewDesk(
        decide: { broker.decide($0) }, stop: { stopped.append($0) },
        decided: { told.append(($0, $1, $2)) })
      desk.now = { clock }
      desk.presenter = shown
      // What the broker hears goes to the desk, as the bridge does.
      func relay(_ type: String) {
        if let event = heard.wait({ if case .message = $0 { true } else { false } }) {
          if case .message(_, let json) = event, String(decoding: json, as: UTF8.self).contains(type) { desk.handle(event) }
        }
      }
      desk.bound(id, agent: "Rivet")
      peer.frame(#"{"type":"hello","v":1,"tab":7,"origin":"https://example.com","title":"Example"}"#)
      relay("hello")
      func review(_ command: Int, _ hash: String, in ms: Int64 = 60_000, name: String = "Send") {
        let expires = Int64(clock * 1000) + ms
        peer.frame(#"{"type":"review","tab":7,"id":\#(command),"hash":"\#(hash)","expires_ms":\#(expires),"action":{"op":"click","origin":"https://example.com","path":"/compose","role":"button","name":"\#(name)","warnings":["may_ask_permission"]}}"#)
        relay("review")
      }
      review(5, "ab")
      check(shown.open[id]?.agent == "Rivet" && shown.open[id]?.origin == "https://example.com", "the panel says who asked and where, from the session")
      check(shown.open[id]?.rows.first { $0.key == "target" }?.value == "button “Send”", "the page's name for the target is quoted")
      check(shown.open[id]?.warnings == ["warning_may_ask_permission"], "and its warnings shown")
      desk.press(id, .allow)
      clock += 0.5
      desk.press(id, .allow)
      desk.press(id, .deny)
      check(told.isEmpty && peer.read(within: 0.3) == nil, "buttons pressed within the first second do nothing")
      review(5, "ab", name: "Delete")
      clock += 0.6
      desk.press(id, .allow)
      check(told.isEmpty, "a changed review waits its second again")
      clock += 0.5
      desk.press(id, .deny)
      let sent = peer.read().flatMap { try? JSONSerialization.jsonObject(with: Data($0.utf8)) as? NSDictionary }
      check(sent == ["type": "decision", "id": 5, "hash": "ab", "allow": false] as NSDictionary, "once armed, Escape or Deny sends the decision")
      check(told.count == 1 && told[0] == (id, 5, false), "and Dart hears what was decided")
      check(shown.open[id] == nil, "the panel closes")
      desk.press(id, .allow)
      check(told.count == 1 && peer.read(within: 0.3) == nil, "once")
      review(6, "cd")
      clock += 1.1
      desk.press(id, .allow)
      let allowed = peer.read().flatMap { try? JSONSerialization.jsonObject(with: Data($0.utf8)) as? NSDictionary }
      check(allowed?["allow"] as? Bool == true, "Allow once sends allow")
      check(desk.state(id) == .allowed, "and the panel says to go back to the tab")
      peer.frame(#"{"type":"result","tab":7,"id":6,"result":{"done":true}}"#)
      relay("result")
      check(shown.open[id] == nil, "the command's result closes it")
      review(7, "ef", in: 5_000)
      clock += 6
      desk.tick()
      check(desk.state(id) == .expired && shown.states[id] == .expired, "a lapsed review says Expired")
      desk.press(id, .allow)
      desk.press(id, .deny)
      check(told.count == 2 && peer.read(within: 0.3) == nil, "and its buttons do nothing")
      clock += 2
      desk.tick()
      check(shown.open[id] == nil, "then it closes")
      review(8, "01")
      clock += 1.1
      desk.press(id, .stop)
      check(stopped == [id] && shown.open[id] == nil, "Stop ends the session and closes it")
      broker.end(id, code: "stop")
      _ = gone(id)
    }

    // No channel method makes a decision: the bridge never presses a panel
    // or makes a decision itself.
    do {
      let here = URL(fileURLWithPath: #filePath).absoluteURL.standardizedFileURL.deletingLastPathComponent().deletingLastPathComponent()
      let bridge = (try? String(contentsOf: here.appendingPathComponent("Runner/BrowserBridge.swift"), encoding: .utf8)) ?? ""
      check(!bridge.isEmpty, "the bridge's source is there to check")
      let code = bridge.split(separator: "\n").filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("//") }.joined(separator: "\n")
      check(!code.contains(".press(") && !code.contains("ReviewClick(") && !code.contains("\"decide\""), "the bridge has no way to decide a review")
    }

    // What closes a helper.
    do {
      let peer = Peer(path)!
      peer.frame(#"{"type":"result"}"#)
      check(goneAny() == "unexpected: its first frame isn't hello", "a first frame that isn't hello closes it")
      check(peer.read() == nil, "closed")
    }
    do {
      let peer = Peer(path)!
      peer.hello(v: 2)
      check(goneAny() == "version: version 2", "another version closes it")
    }
    do {
      broker.appVersion = "v0.1.17"
      let peer = Peer(path)!
      peer.hello(version: "v0.1.16")
      check(goneAny() == "version: helper v0.1.16, app v0.1.17", "another release's helper closes it")
      check(peer.read() == nil, "before any pairing code is sent")
      broker.appVersion = ""
    }
    do {
      let peer = Peer(path)!
      check(goneAny() == "unexpected: no hello within 0 s", "a peer silent past its time is closed")
      check(peer.read() == nil, "closed")
    }
    do {
      let peer = Peer(path)!
      guard let id = connected(peer) else { return check(false, "connects") }
      // 300 KiB said: closed before anything is read or allocated.
      var length = UInt32(300 * 1024).littleEndian
      peer.raw(withUnsafeBytes(of: &length) { Data($0) })
      check(gone(id) == "unexpected: a frame of \(300 * 1024) bytes", "a frame past 256 KiB closes it")
    }
    do {
      let peer = Peer(path)!
      guard let id = connected(peer) else { return check(false, "connects") }
      peer.frame("[1,2]")
      check(gone(id) == "unexpected: a frame that isn't a JSON object", "a frame that isn't a JSON object closes it")
    }
    do {
      let peer = Peer(path)!
      guard let id = connected(peer) else { return check(false, "connects") }
      check(gone(id) == "not_chosen: not chosen within 1 s", "a helper nobody chooses is closed in time")
      check(peer.read() == nil, "closed")
    }
    do {
      let peer = Peer(path)!
      guard let id = connected(peer) else { return check(false, "connects") }
      broker.stop()
      check(gone(id) == "stopped", "stopping closes every helper")
      check(Peer(path) == nil, "and nothing listens after")
    }

    // Versions: only two releases' are compared.
    check(BrowserBroker.versionsDiffer(helper: "v0.1.16", app: "v0.1.17"), "two releases differ")
    check(!BrowserBroker.versionsDiffer(helper: "v0.1.17", app: "v0.1.17"), "the same release doesn't")
    check(!BrowserBroker.versionsDiffer(helper: "0.1.0", app: "v0.1.17"), "a plain build's helper isn't compared")
    check(!BrowserBroker.versionsDiffer(helper: "v0.1.17", app: ""), "nor an app that says none")

    // The fields as the extension sends them: marks and leaves as lists of
    // codes, for the warning box and the destination.
    do {
      let frame = try! JSONSerialization.jsonObject(with: Data(#"{"type":"review","id":1,"hash":"h","expires_ms":1,"action":{"op":"click","path":"/","role":"link","name":"Pay⟦U+202E⟧","destination":"https://other.example/x","leaves":"other_origin","marks":["bidi_control","mixed_script"],"warnings":["may_ask_permission","ends_session"]}}"#.utf8)) as! [String: Any]
      let content = ReviewContent.parse(frame, agent: "Rivet", origin: "https://example.com")
      check(content?.marks == ["mark_bidi_control", "mark_mixed_script"], "a name's marks go to the warning box")
      check(content?.leaves == ["leaves_other_origin"], "leaving for another site is said with where it goes")
      check(content?.warnings == ["warning_may_ask_permission", "warning_ends_session"], "the warnings, in order")
      check(content?.rows.first { $0.key == "destination" }?.value == "https://other.example/x", "and where it goes")
      var newer = frame
      var action = frame["action"] as! [String: Any]
      action["frame"] = "a newer extension's field"
      newer["action"] = action
      check(ReviewContent.parse(newer, agent: "Rivet", origin: "https://example.com")?.warnings.last == "unshown_details", "a field this version can't show is said in the warning box")
    }

    // What a fill types: each ⏎ kept and breaking the line; anything
    // hidden shown.
    check(ReviewContent.typed("Hi⏎⏎there\u{202E}") == "Hi⏎\n⏎\nthere⟦U+202E⟧", "a fill's text shows its line breaks and hidden characters")
    check(ReviewContent.quoted("Send\u{200B} now\n") == "“Send⟦U+200B⟧ now⏎”", "a page's name is one line, quoted")
    check(ReviewContent.ascii("https://exämple.com") == "https://ex⟦U+00E4⟧mple.com", "the origin shows as ASCII")

    // Chrome's manifest at launch: only one the person chose, ours and
    // there, naming another copy's helper, is repaired; nothing else.
    do {
      let home = root.appendingPathComponent("home")
      let file = ChromeManifest.file(dev: true, home: home)
      let host = ChromeManifest.host(dev: true)
      let folder = file.deletingLastPathComponent()
      try! FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
      let app = root.appendingPathComponent("Claktory Dev.app/Contents/Helpers")
      try! FileManager.default.createDirectory(at: app, withIntermediateDirectories: true)
      let helper = app.appendingPathComponent(BrowserBroker.helperName)
      FileManager.default.createFile(atPath: helper.path, contents: Data())
      func write(_ object: [String: Any], to url: URL = file) {
        try! JSONSerialization.data(withJSONObject: object).write(to: url)
      }
      let listing = { try! FileManager.default.contentsOfDirectory(atPath: folder.path).sorted() }
      check(!ChromeManifest.needsRepair(file, host: host, helper: helper), "no manifest (never set up, or turned off): nothing runs")
      check(listing().isEmpty, "and nothing is written")
      write(["name": host, "path": ChromeManifest.realPath(helper), "allowed_origins": ["chrome-extension://abc/"]])
      check(!ChromeManifest.needsRepair(file, host: host, helper: helper), "one naming this helper is left as it is")
      write(["name": host, "path": "/Applications/Old Claktory Dev.app/Contents/Helpers/claktory-browser-host"])
      check(ChromeManifest.needsRepair(file, host: host, helper: helper), "one naming another copy's helper is repaired")
      write(["name": "com.other.host", "path": "/elsewhere"])
      check(!ChromeManifest.needsRepair(file, host: host, helper: helper), "another host's file isn't ours")
      try! FileManager.default.removeItem(at: file)
      let elsewhere = root.appendingPathComponent("elsewhere.json")
      write(["name": host, "path": "/elsewhere"], to: elsewhere)
      try! FileManager.default.createSymbolicLink(at: file, withDestinationURL: elsewhere)
      check(!ChromeManifest.needsRepair(file, host: host, helper: helper), "a link is never followed")
      check(ChromeManifest.registered(file, host: host) == nil, "nor counted as registered")
      check(ChromeManifest.file(dev: false, home: home).lastPathComponent == "com.claktory.browser.json", "the real app's host is its own")
    }

    print("\(names.count - failures)/\(names.count) checks passed")
    exit(failures == 0 ? 0 : 1)
  }
}
