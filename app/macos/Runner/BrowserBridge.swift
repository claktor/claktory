// Browser control's broker (BrowserBroker.swift) on the factory/browser
// channel, for the app's Dart side (Polnareff's routing). Off until Dart
// calls start, which it does only while browser control is on for this Mac;
// nothing here starts, registers or opens anything by itself, except that
// at launch a Chrome manifest the person already chose, pointing at
// another copy's helper (the app moved or was replaced), is repaired.
//
// Dart → native:
//   start {version, words} → {path} | error {code: a BrokerCode, message:
//     detail} (version: the app's FACTORY_VERSION; a helper of another is
//     closed, gone with code version. words: the review panel's fixed
//     labels, Trish's, by ReviewWords.keys)
//   register {replace} → {said} | error {code: register_failed, message}
//     (Chrome's manifest for this app's helper, once the person agreed;
//     replace: they chose to replace another program's file of that name)
//   unregister     → {said} | error {code: register_failed, message}
//   registered     → bool (Chrome's manifest for this flavor is there)
//   stop
//   send {id, json} → bool (false: not a JSON object, too big, a
//     decision, or gone)
//   choose {id}    → bool (the person chose this helper: it stays)
//   end {id, code}  (local Stop: says stop to the helper, then closes)
//   showReview {id} → bool (brings helper id's review panel in front;
//     false when none is open)
// There's no way to decide a review from here: only the person's click in
// the native review panel (BrowserReviewPanel.swift) does.
// native → Dart:
//   helperConnected {id, version, caller, code} (code: the pairing digits
//     the app sent the helper for its popup; show them beside it)
//   helperMessage {id, json}
//   helperGone {id, code, detail}
//   refused {code, detail}
//   decided {id, command, allow} (the person decided in the review panel;
//     Stop there ends the helper: helperGone with code stop)
// Codes (BrokerCode in BrowserBroker.swift), worded in Dart: start's
// unsigned, dev_no_helper, path_too_long, folder_unsafe, cant_listen; a
// peer's not_helper; a helper's closed, unexpected, version, not_chosen,
// stopped, or end's own code. Details are technical, shown faintly.
import FlutterMacOS
import Foundation

final class BrowserBridge {
  private static var shared: BrowserBridge?

  static func register(with messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: "factory/browser", binaryMessenger: messenger)
    let bridge = BrowserBridge(channel: channel)
    shared = bridge
    channel.setMethodCallHandler { call, result in bridge.handle(call, result) }
    bridge.work.async { bridge.repairManifest() }
  }

  private let channel: FlutterMethodChannel
  private var broker: BrowserBroker?
  private let work = DispatchQueue(label: "factory.browser.bridge")

  /// The review panels, on the main thread: opened and closed by the
  /// broker's events, decided only by the person's click.
  private lazy var desk: ReviewDesk = {
    let desk = ReviewDesk(
      decide: { [weak self] click in
        self?.deskBroker?.decide(click) ?? false
      },
      stop: { [weak self] id in
        guard let self else { return }
        self.work.async { self.broker?.end(id, code: BrokerCode.stop) }
      },
      decided: { [weak self] id, command, allow in
        self?.channel.invokeMethod(
          "decided", arguments: ["id": id, "command": command, "allow": allow])
      })
    return desk
  }()
  private lazy var panels = BrowserReviewPanels(desk: desk)
  private var ticking: Timer?
  /// The broker, as the main thread reads it (set once it's made).
  private var deskBroker: BrowserBroker?

  private init(channel: FlutterMethodChannel) {
    self.channel = channel
  }

  /// A broker event, on the main thread: to the review desk, then to Dart.
  private func heard(_ event: BrowserBrokerEvent) {
    if desk.presenter == nil { desk.presenter = panels }
    desk.handle(event)
    if desk.isOpen, ticking == nil {
      ticking = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] timer in
        guard let self else { return timer.invalidate() }
        self.desk.tick()
        if !self.desk.isOpen {
          timer.invalidate()
          self.ticking = nil
        }
      }
    }
    tell(event)
  }

  /// The real app's folder, or Claktory Dev's (any other bundle id): a Dev
  /// app never listens where the real one does.
  private static var dev: Bool { Bundle.main.bundleIdentifier != "dev.factory.app" }

  private func handle(_ call: FlutterMethodCall, _ result: @escaping FlutterResult) {
    let args = call.arguments as? [String: Any] ?? [:]
    // The panels live on the main thread, where this is called.
    if call.method == "showReview" {
      guard let id = args["id"] as? Int else { return result(false) }
      return result(desk.bringForward(id))
    }
    work.async {
      let answer = self.answer(call.method, args)
      DispatchQueue.main.async { result(answer) }
    }
  }

  private func answer(_ method: String, _ args: [String: Any]) -> Any? {
    switch method {
    case "start":
      if broker == nil {
        switch HelperRequirement.forThisApp() {
        case .failure(let why):
          return FlutterError(code: why.code, message: why.detail, details: nil)
        case .success(let requirement):
          broker = BrowserBroker(
            folder: BrowserBroker.folder(dev: BrowserBridge.dev), requirement: requirement
          ) { [weak self] event in DispatchQueue.main.async { self?.heard(event) } }
          let made = broker
          DispatchQueue.main.async { self.deskBroker = made }
        }
      }
      broker!.appVersion = args["version"] as? String ?? ""
      let words = args["words"] as? [String: String] ?? [:]
      DispatchQueue.main.async { self.desk.words = ReviewWords(words: words) }
      switch broker!.start() {
      case .success(let path): return ["path": path]
      case .failure(let why):
        return FlutterError(code: why.code, message: why.detail, details: nil)
      }
    case "stop":
      broker?.stop()
      return nil
    case "send":
      guard let id = args["id"] as? Int, let json = args["json"] as? String else { return false }
      let sent = broker?.send(id, json: Data(json.utf8)) ?? false
      // Who the review panel says asked: the agent this helper is bound to.
      if sent,
        let frame = (try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any],
        frame["type"] as? String == "bound", let agent = frame["agent"] as? String
      {
        DispatchQueue.main.async { self.desk.bound(id, agent: agent) }
      }
      return sent
    case "choose":
      guard let id = args["id"] as? Int else { return false }
      return broker?.choose(id) ?? false
    case "end":
      guard let id = args["id"] as? Int else { return nil }
      broker?.end(id, code: args["code"] as? String ?? BrokerCode.stopped)
      return nil
    case "register":
      var options = ["--register"]
      if args["replace"] as? Bool == true { options.append("--replace") }
      return runHelper(options)
    case "unregister":
      // A manifest left naming another copy's helper (the app moved) isn't
      // this helper's to remove: pointed here first, then removed.
      if ChromeManifest.needsRepair(manifest, host: host, helper: helper) {
        if let failed = runHelper(["--register"]) as? FlutterError { return failed }
      }
      return runHelper(["--unregister"])
    case "registered":
      return ChromeManifest.registered(manifest, host: host) != nil
    default:
      return FlutterMethodNotImplemented
    }
  }

  // MARK: Chrome's manifest

  private var host: String { ChromeManifest.host(dev: BrowserBridge.dev) }
  private var manifest: URL { ChromeManifest.file(dev: BrowserBridge.dev) }
  private var helper: URL {
    Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/\(BrowserBroker.helperName)")
  }

  /// At launch: a manifest the person chose before, naming another copy's
  /// helper, is pointed at this one. None there, nothing runs.
  private func repairManifest() {
    guard ChromeManifest.needsRepair(manifest, host: host, helper: helper),
      FileManager.default.isExecutableFile(atPath: helper.path)
    else { return }
    if let failed = runHelper(["--register"]) as? FlutterError {
      NSLog("Browser control: Chrome's manifest couldn't be repaired: %@", failed.message ?? "")
    }
  }

  /// Runs the app's own helper with [options]: what it said, or an error.
  private func runHelper(_ options: [String]) -> Any {
    let process = Process()
    process.executableURL = helper
    process.arguments = options
    let out = Pipe()
    let err = Pipe()
    process.standardOutput = out
    process.standardError = err
    process.standardInput = FileHandle.nullDevice
    do {
      try process.run()
    } catch {
      return FlutterError(code: "register_failed", message: "\(error.localizedDescription)", details: nil)
    }
    let said = out.fileHandleForReading.readDataToEndOfFile()
    let why = err.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    let text = { (data: Data) in
      String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
    }
    guard process.terminationStatus == 0 else {
      return FlutterError(code: "register_failed", message: text(why), details: nil)
    }
    return ["said": text(said)]
  }

  private func tell(_ event: BrowserBrokerEvent) {
    let (method, args): (String, [String: Any]) =
      switch event {
      case .connected(let id, let version, let caller, let code):
        ("helperConnected", ["id": id, "version": version, "caller": caller, "code": code])
      case .message(let id, let json):
        ("helperMessage", ["id": id, "json": String(decoding: json, as: UTF8.self)])
      case .gone(let id, let code, let detail):
        ("helperGone", ["id": id, "code": code, "detail": detail])
      case .refused(let code, let detail):
        ("refused", ["code": code, "detail": detail])
      }
    DispatchQueue.main.async { self.channel.invokeMethod(method, arguments: args) }
  }
}
