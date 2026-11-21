import AVFoundation
import CoreImage
import Flutter
import Network
import Testing

@testable import Runner

struct RunnerTests {
  /// The camera's reading of QR codes, on an image of a pairing code drawn
  /// as the Mac draws it: the Simulator has no camera.
  @Test func readsAPairingCodeFromAQRImage() throws {
    let code =
      "factory://?t=0000000000000000000000000000000000000000000000000000000000000000"
      + "&name=standin&e=4fb375e4e5877d51fe09e40635833e932b26eb4eed4dc9cdacd1a64f678e3255"
      + "&r=https://euc1-1.relay.n0.iroh.link./"
    let filter = try #require(CIFilter(name: "CIQRCodeGenerator"))
    filter.setValue(Data(code.utf8), forKey: "inputMessage")
    filter.setValue("M", forKey: "inputCorrectionLevel")
    let modules = try #require(filter.outputImage)
    let scaled = modules.transformed(by: CGAffineTransform(scaleX: 8, y: 8))
    // Dark modules on white with a quiet zone around them.
    let white = CIImage(color: .white).cropped(to: scaled.extent.insetBy(dx: -64, dy: -64))
    let image = scaled.composited(over: white)
    let picture = try #require(CIContext().createCGImage(image, from: image.extent))
    #expect(CodeReader.read(picture) == [code])
  }

  @Test func readsNothingWithoutACode() throws {
    let blank = CIImage(color: .white).cropped(to: CGRect(x: 0, y: 0, width: 400, height: 400))
    let picture = try #require(CIContext().createCGImage(blank, from: blank.extent))
    #expect(CodeReader.read(picture).isEmpty)
  }
}

/// Stands in for Flutter's messenger: keeps the handlers the native side
/// sets, so a test can call a channel as the app would and hear its events.
final class FakeMessenger: NSObject, FlutterBinaryMessenger {
  var handlers: [String: FlutterBinaryMessageHandler] = [:]
  var sent: [String: [Data]] = [:]
  private var next: FlutterBinaryMessengerConnection = 0

  func send(onChannel channel: String, message: Data?) {
    if let message { sent[channel, default: []].append(message) }
  }

  func send(
    onChannel channel: String, message: Data?, binaryReply callback: FlutterBinaryReply? = nil
  ) {
    send(onChannel: channel, message: message)
    callback?(nil)
  }

  func setMessageHandlerOnChannel(
    _ channel: String, binaryMessageHandler handler: FlutterBinaryMessageHandler? = nil
  ) -> FlutterBinaryMessengerConnection {
    handlers[channel] = handler
    next += 1
    return next
  }

  func cleanUpConnection(_ connection: FlutterBinaryMessengerConnection) {}

  /// Calls [method] on [channel] as the app would; its result or error,
  /// or FlutterMethodNotImplemented for a method the channel doesn't have
  /// (which replies with nothing at all).
  func call(_ channel: String, _ method: String, _ arguments: Any? = nil) async -> Any? {
    let codec = FlutterStandardMethodCodec.sharedInstance()
    let message = codec.encode(FlutterMethodCall(methodName: method, arguments: arguments))
    let handler = handlers[channel]!
    return await withCheckedContinuation { done in
      handler(message) { reply in
        guard let reply else { return done.resume(returning: FlutterMethodNotImplemented) }
        done.resume(returning: Self.decode(reply))
      }
    }
  }

  /// A result, or the FlutterError it failed with.
  private static func decode(_ reply: Data) -> Any? {
    let codec = FlutterStandardMethodCodec.sharedInstance()
    let value = codec.decodeEnvelope(reply)
    return value is NSNull ? nil : value
  }

  /// Starts listening on an event channel; the events, as they come.
  func listen(_ channel: String) {
    let codec = FlutterStandardMethodCodec.sharedInstance()
    handlers[channel]!(codec.encode(FlutterMethodCall(methodName: "listen", arguments: nil))) { _ in
    }
  }

  func events(_ channel: String) -> [Any] {
    let codec = FlutterStandardMethodCodec.sharedInstance()
    return (sent[channel] ?? []).compactMap { codec.decodeEnvelope($0) }
  }
}

/// Voice messages on the iPhone (#197): the recorder and the player the Mac
/// shares, on their channels. The Simulator's microphone isn't allowed in
/// a test, so recording itself is the Mac's and the integration test's to
/// check; these cover what the app relies on around it.
@MainActor
struct VoiceTests {
  @Test func theRecorderAnswersOnItsChannel() async throws {
    let messenger = FakeMessenger()
    Recorder.register(with: messenger)
    let access = await messenger.call("factory/recorder", "access") as? String
    #expect(["granted", "denied", "undetermined", "restricted"].contains(access))
    // Nothing recording: stop and cancel are harmless.
    #expect(await messenger.call("factory/recorder", "stop") == nil)
    #expect(await messenger.call("factory/recorder", "cancel") == nil)
    if access != "granted" {
      let path = NSTemporaryDirectory() + "voice-\(UUID()).m4a"
      let refused = await messenger.call("factory/recorder", "start", path) as? FlutterError
      #expect(refused?.code == "denied")
    }
    let unknown = await messenger.call("factory/recorder", "nonsense")
    #expect(unknown as? NSObject === FlutterMethodNotImplemented)
  }

  @Test func thePlayerPlaysARecordingAndSaysWhere() async throws {
    let path = try Self.tone(seconds: 1)
    let messenger = FakeMessenger()
    AudioPlayer.register(with: messenger)
    messenger.listen("factory/audio/events")
    let started = await messenger.call("factory/audio", "play", ["path": path, "fromMs": 0])
    #expect(started == nil)
    try await Task.sleep(nanoseconds: 300_000_000)
    let playing = try #require(messenger.events("factory/audio/events").last as? [String: Any])
    #expect(playing["path"] as? String == path)
    #expect(playing["state"] as? String == "playing")
    let duration = try #require(playing["durationMs"] as? Int)
    #expect((900...1100).contains(duration))

    _ = await messenger.call("factory/audio", "pause")
    let paused = messenger.events("factory/audio/events").last as? [String: Any]
    #expect(paused?["state"] as? String == "paused")
    _ = await messenger.call("factory/audio", "stop")
    let stopped = messenger.events("factory/audio/events").last as? [String: Any]
    #expect(stopped?["state"] as? String == "stopped")
  }

  @Test func thePlayerRefusesWhatItCantPlay() async throws {
    let messenger = FakeMessenger()
    AudioPlayer.register(with: messenger)
    let missing = await messenger.call(
      "factory/audio", "play", ["path": "/nowhere/voice.m4a", "fromMs": 0])
    #expect((missing as? FlutterError)?.code == "failed")
  }

  /// A second of a tone, as the recorder writes them: AAC in .m4a, 16 kHz
  /// mono.
  static func tone(seconds: Double) throws -> String {
    let path = NSTemporaryDirectory() + "tone-\(UUID()).m4a"
    let settings: [String: Any] = [
      AVFormatIDKey: kAudioFormatMPEG4AAC,
      AVSampleRateKey: 16_000,
      AVNumberOfChannelsKey: 1,
      AVEncoderBitRateKey: 32_000,
    ]
    let file = try AVAudioFile(forWriting: URL(fileURLWithPath: path), settings: settings)
    let format = file.processingFormat
    let frames = AVAudioFrameCount(seconds * format.sampleRate)
    let buffer = try #require(AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frames))
    buffer.frameLength = frames
    let samples = try #require(buffer.floatChannelData?[0])
    for i in 0..<Int(frames) {
      samples[i] = 0.3 * sin(2 * .pi * 440 * Float(i) / Float(format.sampleRate))
    }
    try file.write(from: buffer)
    return path
  }
}

/// The network path (#214), the Mac's shared NetworkPath.swift: listened
/// to, it says the path at once, in the shape app/lib/network.dart reads.
@MainActor
struct NetworkPathTests {
  @Test func saysThePathAtOnceWhenListenedTo() async throws {
    let messenger = FakeMessenger()
    NetworkPath.register(with: messenger)
    messenger.listen("factory/network")
    for _ in 0..<50 where messenger.events("factory/network").isEmpty {
      try await Task.sleep(nanoseconds: 50_000_000)
    }
    let path = try #require(messenger.events("factory/network").first as? [String: Any])
    #expect(path["satisfied"] is Bool)
    #expect(path["expensive"] is Bool)
    #expect(path["constrained"] is Bool)
    let interfaces = try #require(path["interfaces"] as? [String])
    #expect(interfaces.allSatisfy { ["wifi", "cellular", "wired", "loopback", "other"].contains($0) })
    // The Simulator shares this Mac's network, which is up.
    #expect(path["satisfied"] as? Bool == true)
  }

  @Test func interfaceTypesHaveTheirNames() {
    #expect(NetworkPath.name(of: .wifi) == "wifi")
    #expect(NetworkPath.name(of: .cellular) == "cellular")
    #expect(NetworkPath.name(of: .wiredEthernet) == "wired")
    #expect(NetworkPath.name(of: .loopback) == "loopback")
    #expect(NetworkPath.name(of: .other) == "other")
  }
}

/// What the device is (#iPad hook 4, Capabilities.swift), in the shape
/// docs/ipad-workbench.md §3 agreed: from UIKit, never the window.
@MainActor
struct CapabilitiesTests {
  @Test func saysWhatThisDeviceIs() throws {
    let caps = Capabilities.current()
    #expect(caps["schema_version"] as? Int == 1)
    let idiom = try #require(caps["idiom"] as? String)
    #expect(idiom == Capabilities.idiom(UIDevice.current.userInterfaceIdiom))
    #expect(["pad", "phone", "mac", "unspecified"].contains(idiom))
    #expect(caps["ios_app_on_mac"] as? Bool == false)
    #expect(caps["os_version"] as? String == UIDevice.current.systemVersion)
    #expect(caps["hardware_keyboard"] is Bool)
    let pencil = try #require(caps["pencil"] as? [String: Bool])
    #expect(Set(pencil.keys) == ["hover", "squeeze", "double_tap"])
    // A phone has no Pencil to offer.
    if idiom == "phone" { #expect(pencil.values.allSatisfy { !$0 }) }
    let overlay = try #require(caps["pdf_page_overlay"] as? [String: Any])
    #expect(overlay["available"] as? Bool == true)
    #expect(overlay["min_os"] as? String == "16.0")
    let strokes = try #require(caps["stroke_recognizer"] as? [String: Any])
    #expect(strokes["min_os"] as? String == "27.0")
    #expect(strokes["available"] is Bool)
  }

  /// The same keys and kinds of value as Yuki's fixtures, which the
  /// Dart side reads (app/test/ipad/fixtures/bridge/).
  @Test func hasTheFixturesShape() throws {
    let bridge = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent().appendingPathComponent("../../test/ipad/fixtures/bridge")
    func shape(_ value: Any) -> Any {
      switch value {
      case let map as [String: Any]: return map.mapValues(shape)
      case is Bool: return "bool"
      case is String: return "string"
      case is Int: return "number"
      default: return "\(type(of: value))"
      }
    }
    let ours = shape(Capabilities.current()) as! [String: Any]
    for name in ["capabilities_phone", "capabilities_pad", "capabilities_pad_ios15"] {
      let data = try Data(contentsOf: bridge.appendingPathComponent("\(name).json"))
      let fixture = try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
      // JSON's true and 1 are both NSNumber: compare through the fixture's
      // own kinds, booleans told apart by their Objective-C type.
      func kinds(_ value: Any) -> Any {
        switch value {
        case let map as [String: Any]: return map.mapValues(kinds)
        case let number as NSNumber:
          return CFGetTypeID(number) == CFBooleanGetTypeID() ? "bool" : "number"
        case is String: return "string"
        default: return "\(type(of: value))"
        }
      }
      #expect(
        NSDictionary(dictionary: kinds(fixture) as! [String: Any]).isEqual(to: ours), "\(name)")
    }
    let keyboard = try Data(contentsOf: bridge.appendingPathComponent("keyboard_changed.json"))
    let event = try #require(
      (try JSONSerialization.jsonObject(with: keyboard) as? [String: Any])?["event"]
        as? [String: Any])
    #expect(Set(event.keys) == ["hardware_keyboard"])
  }

  @Test func idiomsHaveTheirNames() {
    #expect(Capabilities.idiom(.pad) == "pad")
    #expect(Capabilities.idiom(.phone) == "phone")
    #expect(Capabilities.idiom(.mac) == "mac")
    #expect(Capabilities.idiom(.tv) == "unspecified")
  }
}

/// The iPhone's connections (the Mac's ConnectionStore.swift): a keychain
/// it can't read is an error, never an empty store, and isn't written over.
struct ConnectionStoreTests {
  final class Locked: SecretStore {
    var data: Data? = try! ConnectionStore.encode(["current": "factory://?t=x"])
    var locked = true
    func read() throws -> Data? {
      if locked { throw KeychainError(status: errSecInteractionNotAllowed) }
      return data
    }
    func write(_ data: Data) throws { self.data = data }
    func delete() throws { data = nil }
  }

  func code(_ run: () throws -> Void) -> String? {
    do {
      try run()
      return nil
    } catch {
      return (error as? StoreFailure)?.code ?? "\(error)"
    }
  }

  @Test func aLockedKeychainIsAnErrorAndIsntWrittenOver() throws {
    let keychain = Locked()
    let store = ConnectionStore(
      defaults: UserDefaults(suiteName: "claktory-check-\(UUID())")!, keychain: keychain,
      fallsBack: false, log: { _ in })
    #expect(code { _ = try store.read() } == "credentials_unavailable")
    #expect(code { try store.write([:]) } == "credentials_read_failed")
    #expect(keychain.data != nil)
    keychain.locked = false
    #expect(try store.read()["current"] as? String == "factory://?t=x")
    try store.write(["current": ""])
    try store.reset()
    #expect(keychain.data == nil)
    #expect(try store.read().isEmpty)
  }

  @Test func theRealKeychainKeepsAnItemAfterFirstUnlock() throws {
    let item = KeychainItem(
      service: "Claktory connections check", account: UUID().uuidString, label: nil,
      accessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly)
    defer { try? item.delete() }
    #expect(try item.read() == nil)
    try item.write(Data("one".utf8))
    try item.write(Data("two".utf8))
    #expect(try item.read() == Data("two".utf8))
    try item.delete()
    #expect(try item.read() == nil)
  }
}

/// The iPad workbench's commands in the system's menu (#209, Petra's
/// app/lib/ipad/commands.dart), against her fixtures in
/// app/test/ipad/fixtures/commands/.
@MainActor
struct CommandMenuTests {
  static func fixture(_ name: String) throws -> [String: Any] {
    let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .appendingPathComponent("../../test/ipad/fixtures/commands/\(name).json")
    let data = try Data(contentsOf: url)
    return try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
  }

  /// A menu on a fake messenger, counting the rebuilds it asks for.
  static func menu() -> (CommandMenu, FakeMessenger, () -> Int) {
    let messenger = FakeMessenger()
    let menu = CommandMenu()
    var rebuilds = 0
    menu.rebuild = { rebuilds += 1 }
    menu.attach(FlutterMethodChannel(name: CommandMenu.channelName, binaryMessenger: messenger))
    return (menu, messenger, { rebuilds })
  }

  @Test func takesTheFixturesCommandsAsKeyCommands() async throws {
    let set = try Self.fixture("set_commands")
    #expect(set["channel"] as? String == CommandMenu.channelName)
    let (menu, messenger, rebuilds) = Self.menu()
    let answer = await messenger.call(CommandMenu.channelName, "setCommands", set["arguments"])
    #expect(answer as? Bool == set["result"] as? Bool)
    #expect(rebuilds() == 1)

    let keys = menu.keyCommands(action: #selector(AppDelegate.runWorkbenchCommand(_:)))
    #expect(keys.map(\.title) == ["Hide Sidebar", "Next Pane", "Previous Pane", "Panes…"])
    #expect(keys.map { $0.propertyList as? String } == ["sidebar", "next-pane", "previous-pane", "panes"])
    #expect(keys.map(\.input) == ["s", "\t", "\t", "p"])
    #expect(keys[0].modifierFlags == [.command, .control])
    #expect(keys[1].modifierFlags == [.control])
    #expect(keys[2].modifierFlags == [.control, .shift])
    #expect(keys[3].modifierFlags == [.command, .alternate])
    #expect(keys.filter { !$0.wantsPriorityOverSystemBehavior }.isEmpty)
    #expect(menu.isEnabled("sidebar"))
    #expect(!menu.isEnabled("nonsense"))
    #expect(!menu.isEnabled(nil))
  }

  @Test func rebuildsOnlyWhenWhatItShowsChanges() async throws {
    let (menu, messenger, rebuilds) = Self.menu()
    var arguments = try #require(try Self.fixture("set_commands")["arguments"] as? [String: Any])
    _ = await messenger.call(CommandMenu.channelName, "setCommands", arguments)
    _ = await messenger.call(CommandMenu.channelName, "setCommands", arguments)
    #expect(rebuilds() == 1)

    // Disabled: asked each time it's shown or pressed, no rebuild.
    var commands = try #require(arguments["commands"] as? [[String: Any]])
    commands[1]["enabled"] = false
    arguments["commands"] = commands
    _ = await messenger.call(CommandMenu.channelName, "setCommands", arguments)
    #expect(rebuilds() == 1)
    #expect(!menu.isEnabled("next-pane"))

    // A new title is a new menu.
    commands[0]["title"] = "Show Sidebar"
    arguments["commands"] = commands
    _ = await messenger.call(CommandMenu.channelName, "setCommands", arguments)
    #expect(rebuilds() == 2)
    #expect(menu.commands.first?.title == "Show Sidebar")
  }

  @Test func runsWhatThePersonChoseAsTheFixtureSays() async throws {
    let run = try Self.fixture("run")
    let (menu, messenger, _) = Self.menu()
    var arguments = try #require(try Self.fixture("set_commands")["arguments"] as? [String: Any])
    var commands = try #require(arguments["commands"] as? [[String: Any]])
    commands[3]["enabled"] = false
    arguments["commands"] = commands
    _ = await messenger.call(CommandMenu.channelName, "setCommands", arguments)

    menu.run("sidebar")
    menu.run("panes")  // Disabled: nothing goes.
    menu.run("nonsense")
    let codec = FlutterStandardMethodCodec.sharedInstance()
    let calls = (messenger.sent[CommandMenu.channelName] ?? []).map(codec.decodeMethodCall)
    #expect(calls.map(\.method) == [run["method"] as? String])
    let sent = NSDictionary(dictionary: calls.first?.arguments as? [String: Any] ?? [:])
    let same = sent.isEqual(to: run["arguments"] as? [String: Any] ?? [:])
    #expect(same)
  }

  @Test func refusesWhatItCantShowAsAsked() async throws {
    let (menu, messenger, rebuilds) = Self.menu()
    let good = try #require(try Self.fixture("set_commands")["arguments"] as? [String: Any])
    _ = await messenger.call(CommandMenu.channelName, "setCommands", good)
    let command: [String: Any] = [
      "id": "x", "title": "X", "input": "x", "modifiers": ["command"], "enabled": true,
    ]
    func with(_ change: (inout [String: Any]) -> Void) -> [String: Any] {
      var one = command
      change(&one)
      return ["schema_version": 1, "commands": [one]]
    }
    let refused: [Any?] = [
      nil,
      ["schema_version": 2, "commands": [command]],
      with { $0["modifiers"] = ["hyper"] },
      with { $0["input"] = UIKeyCommand.inputEscape },
      with { $0["title"] = "" },
      with { $0.removeValue(forKey: "enabled") },
      ["schema_version": 1, "commands": [command, command]],
    ]
    for arguments in refused {
      let answer = await messenger.call(CommandMenu.channelName, "setCommands", arguments)
      #expect(answer as? Bool == false)
    }
    // What it showed stays.
    #expect(menu.commands.count == 4)
    #expect(rebuilds() == 1)
    // None at all is fine: the menu goes.
    let none = await messenger.call(
      CommandMenu.channelName, "setCommands", ["schema_version": 1, "commands": [Any]()])
    #expect(none as? Bool == true)
    #expect(menu.commands.isEmpty)
    #expect(rebuilds() == 2)
    let unknown = await messenger.call(CommandMenu.channelName, "nonsense")
    #expect(unknown as? NSObject === FlutterMethodNotImplemented)
  }
}
