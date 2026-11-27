import AVFoundation
import CoreImage
import Flutter
import Network
import Security
import Testing
import UIKit

@testable import Runner

struct RunnerTests {
  /// The paperclip's menu is a popover on an iPad, which UIKit ends the app
  /// for when it comes from nowhere (Zucchero row 630): it points at the
  /// paperclip's rect, or at the middle with no arrow.
  /// Take Photo's file is named for when it was taken, as a Mac names a
  /// screenshot, so two photos never both read Photo.jpg.
  @Test func takenPhotoNamedForItsTime() throws {
    let date = try #require(
      Calendar.current.date(
        from: DateComponents(year: 2026, month: 11, day: 26, hour: 21, minute: 4, second: 33)))
    #expect(Attach.photoTime.string(from: date) == "2026-11-26 at 21.04.33")
  }

  @MainActor @Test func attachMenuHasSomewhereToComeFrom() throws {
    // An iPad's: on an iPhone the menu is a sheet from the bottom, with no
    // popover to point (and RunnerTests run on an iPhone 16 Pro).
    guard UIDevice.current.userInterfaceIdiom == .pad else { return }
    let view = UIView(frame: CGRect(x: 0, y: 0, width: 1024, height: 1366))
    let menu = UIAlertController(title: nil, message: nil, preferredStyle: .actionSheet)
    Attach.anchor(menu, in: view, rect: [900, 1300, 44, 44])
    // An iPhone's action sheet rises from the bottom: no popover to place.
    guard UIDevice.current.userInterfaceIdiom == .pad else {
      #expect(menu.popoverPresentationController == nil)
      return
    }
    let popover = try #require(menu.popoverPresentationController)
    #expect(popover.sourceView === view)
    #expect(popover.sourceRect == CGRect(x: 900, y: 1300, width: 44, height: 44))

    let unplaced = UIAlertController(title: nil, message: nil, preferredStyle: .actionSheet)
    Attach.anchor(unplaced, in: view, rect: nil)
    let middle = try #require(unplaced.popoverPresentationController)
    #expect(middle.sourceView === view)
    #expect(middle.sourceRect == CGRect(x: 512, y: 683, width: 0, height: 0))
    #expect(middle.permittedArrowDirections == [])
  }

  /// The keys of end-to-end direct messages (factory/dm-keys, E6): kept in
  /// the app's own keychain group, given back, replaced and deleted; absent
  /// is nil; names and sizes as Android's; the store's folder.
  @Test func keepsDirectMessageKeys() throws {
    let store = DmKeyStore(
      group: nil, dataProtection: false, service: "com.claktory.dm-keys.test-\(UUID())")
    #expect(store.available())
    #expect(try store.get("identity") == nil)
    let key = Data((0..<32).map { UInt8($0) })
    try store.put("identity", key)
    #expect(try store.get("identity") == key)
    try store.put("identity", Data(repeating: 7, count: 32))
    #expect(try store.get("identity") == Data(repeating: 7, count: 32))
    try store.delete("identity")
    #expect(try store.get("identity") == nil)
    try store.delete("identity")
    #expect(throws: DmKeyFailure.self) { try store.put("Identity", key) }
    #expect(throws: DmKeyFailure.self) {
      try store.put("leaf", Data(count: DmKeyStore.valueLimit + 1))
    }
    try store.put("leaf", Data(count: DmKeyStore.valueLimit))
    try store.delete("leaf")

    // Not in the notifications group the extension shares.
    let shared = try #require(
      Bundle.main.object(forInfoDictionaryKey: "ClaktoryPushKeychainGroup") as? String)
    try store.put("store", key)
    var lookup: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: store.service,
      kSecAttrAccount as String: "store",
      kSecReturnAttributes as String: true,
    ]
    var found: CFTypeRef?
    #expect(SecItemCopyMatching(lookup as CFDictionary, &found) == errSecSuccess)
    let group = (found as? [String: Any])?[kSecAttrAccessGroup as String] as? String
    // Exactly the app's own group, <team>.<bundle id>: the first of
    // Runner.entitlements' keychain-access-groups. Were the notifications
    // group ever first, the extension could read these keys (Fugo).
    let team = try #require(KeychainPushKeys.teamPrefix(of: shared))
    let bundle = try #require(Bundle.main.bundleIdentifier)
    #expect(group == "\(team).\(bundle)")
    #expect(group != shared)
    lookup[kSecAttrSynchronizable as String] = true
    #expect(SecItemCopyMatching(lookup as CFDictionary, &found) == errSecItemNotFound)
    try store.delete("store")

    // The folder: the same every time, never backed up, protected until the
    // first unlock.
    let folder = try DmKeyStore.folder()
    #expect(try DmKeyStore.folder() == folder)
    #expect(folder.hasSuffix("/Library/Application Support/dm"))
    let url = URL(fileURLWithPath: folder)
    #expect(try url.resourceValues(forKeys: [.isExcludedFromBackupKey]).isExcludedFromBackup == true)
    let protection = try FileManager.default.attributesOfItem(atPath: folder)[.protectionKey]
      as? FileProtectionType
    #expect(protection == nil || protection == .completeUntilFirstUserAuthentication)
  }

  /// Copying a secret (factory/secret-clipboard), on a pasteboard of its
  /// own: plain text that expires, its token the write's changeCount, and
  /// cleared only while it still holds that write.
  @Test func copiesASecretAndClearsOnlyItsOwnWrite() throws {
    let board = try #require(UIPasteboard(name: .init("claktory-check-\(UUID())"), create: true))
    defer { UIPasteboard.remove(withName: board.name) }

    let token = SecretClipboard.copy("add-code-123", seconds: 1800, on: board)
    #expect(token == board.changeCount)
    #expect(board.string == "add-code-123")
    SecretClipboard.clear(token: token, on: board)
    #expect(board.string == nil)

    let again = SecretClipboard.copy("add-code-456", seconds: 1800, on: board)
    board.string = "the person's own"
    SecretClipboard.clear(token: again, on: board)
    #expect(board.string == "the person's own")

    // Expired: gone by itself.
    _ = SecretClipboard.copy("add-code-789", seconds: 1, on: board)
    Thread.sleep(forTimeInterval: 1.5)
    #expect(board.string == nil)
  }

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
  /// (which replies with nothing at all). One never answered fails its test
  /// after 30 s, naming the call, and gives nil.
  func call(_ channel: String, _ method: String, _ arguments: Any? = nil) async -> Any? {
    let codec = FlutterStandardMethodCodec.sharedInstance()
    let message = codec.encode(FlutterMethodCall(methodName: method, arguments: arguments))
    let handler = handlers[channel]!
    let answer: Any?? = await InkTests.answer("\(channel) \(method)") { done in
      handler(message) { reply in
        guard let reply else { return done(FlutterMethodNotImplemented) }
        done(Self.decode(reply))
      }
    }
    return answer ?? nil
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

  /// Where recordings are made and kept until sent (#401): in the app's
  /// Application Support, this user's only, never backed up, and readable
  /// after the first unlock, so a send can be retried while locked.
  @Test func theVoiceFolderIsPrivateAndNeverBackedUp() async throws {
    let messenger = FakeMessenger()
    Recorder.register(with: messenger)
    let path = try #require(
      await messenger.call("factory/recorder", "voiceFolder") as? String)
    #expect(path.hasSuffix("/Library/Application Support/Voice not sent"))
    let url = URL(fileURLWithPath: path, isDirectory: true)
    let values = try url.resourceValues(forKeys: [.isExcludedFromBackupKey])
    #expect(values.isExcludedFromBackup == true)
    let attributes = try FileManager.default.attributesOfItem(atPath: path)
    #expect((attributes[.posixPermissions] as? NSNumber)?.intValue == 0o700)
    // The Simulator keeps no file-protection class (it reads nil there),
    // so only a device can show it.
    #if !targetEnvironment(simulator)
      #expect(
        attributes[.protectionKey] as? FileProtectionType
          == .completeUntilFirstUserAuthentication)
    #endif
    // Asked again, the same folder, still so.
    #expect(await messenger.call("factory/recorder", "voiceFolder") as? String == path)
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
    #expect(keys.map(\.title) == ["Hide Sidebar", "Next Pane", "Previous Pane", "Panes…", "Find", "Settings"])
    #expect(keys.map { $0.propertyList as? String } == ["sidebar", "next-pane", "previous-pane", "panes", "find", "settings"])
    #expect(keys.map(\.input) == ["s", "\t", "\t", "p", "f", ","])
    #expect(keys[0].modifierFlags == [.command, .control])
    #expect(keys[1].modifierFlags == [.control])
    #expect(keys[2].modifierFlags == [.control, .shift])
    #expect(keys[3].modifierFlags == [.command, .alternate])
    #expect(keys[4].modifierFlags == [.command])
    #expect(keys[5].modifierFlags == [.command])
    #expect(keys.filter { !$0.wantsPriorityOverSystemBehavior }.isEmpty)
    #expect(menu.isEnabled("sidebar"))
    #expect(!menu.isEnabled("nonsense"))
    #expect(!menu.isEnabled(nil))
  }

  @Test func specialKeysAreTheInputsUIKitNames() throws {
    let inputs = try #require(try Self.fixture("special_keys")["inputs"] as? [String: String])
    #expect(inputs["arrowUp"] == UIKeyCommand.inputUpArrow)
    #expect(inputs["arrowDown"] == UIKeyCommand.inputDownArrow)
    #expect(inputs["arrowLeft"] == UIKeyCommand.inputLeftArrow)
    #expect(inputs["arrowRight"] == UIKeyCommand.inputRightArrow)
    #expect(inputs["enter"] == "\r")
    #expect(inputs["backspace"] == UIKeyCommand.inputDelete)
    #expect(inputs["tab"] == "\t")
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
    #expect(menu.commands.count == 6)
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

/// Links that open the app (AppDelegate.swift's Links): ours wait until the
/// app listens, then come as they open; anything else, or a link longer
/// than any pairing code, never reaches it.
@MainActor
struct LinksTests {
  private func opened(_ messenger: FakeMessenger) -> [String] {
    let codec = FlutterStandardMethodCodec.sharedInstance()
    return (messenger.sent["factory/links"] ?? []).compactMap {
      let call = codec.decodeMethodCall($0)
      return call.method == "opened" ? call.arguments as? String : nil
    }
  }

  @Test func ourLinksWaitThenCome() async throws {
    let links = Links()
    let messenger = FakeMessenger()
    links.register(with: messenger)
    let code = "factory://?t=\(String(repeating: "0", count: 64))&e=\(String(repeating: "a", count: 64))&name=mac"
    #expect(links.open([URL(string: code)!, URL(string: "claktory-dev://add?code=x")!]))
    // Not ours: dropped.
    #expect(!links.open([URL(string: "https://example.com")!, URL(string: "mailto:a@b.c")!]))
    let long = "factory://?t=" + String(repeating: "0", count: Links.maxLength)
    #expect(!links.open([URL(string: long)!]))
    #expect(opened(messenger).isEmpty)
    let waiting = await messenger.call("factory/links", "listen") as? [String]
    #expect(waiting == [code, "claktory-dev://add?code=x"])
    // Listening: each as it comes.
    links.open([URL(string: "claktory://add?code=y")!])
    #expect(opened(messenger) == ["claktory://add?code=y"])
    let unknown = await messenger.call("factory/links", "nonsense")
    #expect(unknown as? NSObject === FlutterMethodNotImplemented)
  }

  /// Claktory's pairing page as a Universal Link (#352 P6): only
  /// https://claktory.com/pair with a code in its fragment; any other page,
  /// host, path, port, user or query is dropped.
  @Test func onlyThePairingPageOpensTheApp() async throws {
    let page = "https://claktory.com/pair#t=\(String(repeating: "0", count: 64))&name=mac"
    #expect(Links.isPairPage(URL(string: page)!))
    #expect(Links.isPairPage(URL(string: "https://CLAKTORY.com/pair#t=1")!))
    for other in [
      "https://claktory.com/pair",
      "https://claktory.com/pair#",
      "https://claktory.com/pairs#t=1",
      "https://claktory.com/pair/x#t=1",
      "https://claktory.com/pair?t=1#t=1",
      "https://claktory.com:8443/pair#t=1",
      "https://a@claktory.com/pair#t=1",
      "https://www.claktory.com/pair#t=1",
      "https://claktory.com.evil.example/pair#t=1",
      "http://claktory.com/pair#t=1",
    ] {
      #expect(!Links.isPairPage(URL(string: other)!), "\(other)")
    }
    let links = Links()
    let messenger = FakeMessenger()
    links.register(with: messenger)
    #expect(!links.open([URL(string: "https://claktory.com/download")!]))
    #expect(links.open([URL(string: page)!]))
    // As the system hands it over: a web-browsing activity.
    let activity = NSUserActivity(activityType: NSUserActivityTypeBrowsingWeb)
    activity.webpageURL = URL(string: page)
    #expect(Links.page(activity) == URL(string: page))
    let other = NSUserActivity(activityType: NSUserActivityTypeBrowsingWeb)
    other.webpageURL = URL(string: "https://claktory.com/changelog")
    #expect(Links.page(other) == nil)
    let handoff = NSUserActivity(activityType: "com.claktory.something")
    handoff.webpageURL = URL(string: page)
    #expect(Links.page(handoff) == nil)
    let waiting = await messenger.call("factory/links", "listen") as? [String]
    #expect(waiting == [page])
  }
}

@MainActor
struct KeptFilesTests {
  /// The files kept from a Claktory on another machine (#446): the folder,
  /// what was in it, and a file moved in all can't be opened while the
  /// iPhone is locked.
  @Test func keptFilesAreProtected() throws {
    let manager = FileManager.default
    let root = NSTemporaryDirectory() + "kept-\(UUID())"
    defer { try? manager.removeItem(atPath: root) }
    let folder = root + "/files"
    let claktory = folder + "/abc"
    try manager.createDirectory(atPath: claktory, withIntermediateDirectories: true)
    let old = claktory + "/old.json"
    #expect(manager.createFile(atPath: old, contents: Data("{}".utf8)))
    // The Simulator reports no class (as DmKeyStore's and InkCache's
    // checks find): there, none; on an iPhone, completeUnlessOpen.
    func protected(_ path: String) throws -> Bool {
      let protection =
        try manager.attributesOfItem(atPath: path)[.protectionKey] as? FileProtectionType
      return protection == nil || protection == .completeUnlessOpen
    }

    try AppDelegate.protectKeptFiles(folder)
    #expect(try protected(folder))
    #expect(try protected(claktory))
    #expect(try protected(old))

    // A copy downloaded elsewhere and moved in keeps its own class until
    // it's protected too.
    let moved = claktory + "/video.mp4"
    let download = root + "/download.mp4"
    #expect(manager.createFile(
      atPath: download, contents: Data([1, 2, 3]),
      attributes: [.protectionKey: FileProtectionType.none]))
    try manager.moveItem(atPath: download, toPath: moved)
    try AppDelegate.protectKeptFiles(moved)
    #expect(try protected(moved))

    // Made where it isn't yet.
    let fresh = root + "/elsewhere/files"
    try AppDelegate.protectKeptFiles(fresh)
    #expect(try protected(fresh))
  }
}

/// History's video export (#458 E1): the writer's channel. A bad argument
/// is an error, never a crash; it writes only to a file it makes itself in
/// the share folder, and deletes it when an export stops.
// Serialized: the app keeps one writer, and registering a test's replaces
// the other's, whose calls then go unanswered.
@Suite(.serialized)
@MainActor
struct ReplayWriterTests {
  let channel = "claktory/replay-writer"

  func code(_ reply: Any?) -> String? { (reply as? FlutterError)?.code }

  func frame(_ bytes: Int) -> [String: Any] {
    ["rgba": FlutterStandardTypedData(bytes: Data(count: bytes))]
  }

  @Test func refusesBadArguments() async throws {
    let messenger = FakeMessenger()
    ReplayWriter.register(with: messenger)
    #expect(code(await messenger.call(channel, "start")) == "bad")
    #expect(code(await messenger.call(channel, "start", ["width": "wide"])) == "bad")
    #expect(code(await messenger.call(channel, "start", ["width": 17, "height": 16, "fps": 30])) == "bad")
    #expect(code(await messenger.call(channel, "start", ["width": 4000, "height": 16, "fps": 30])) == "limit")
    #expect(code(await messenger.call(channel, "start", ["width": 32, "height": 32, "fps": 0])) == "bad")
    #expect(code(await messenger.call(channel, "frame", frame(32 * 32 * 4))) == "none")
    #expect(code(await messenger.call(channel, "frame", ["rgba": "pixels"])) == "bad")
    #expect(code(await messenger.call(channel, "finish")) == "none")
    #expect(await messenger.call(channel, "seek") as? NSObject === FlutterMethodNotImplemented)

    // A frame of the wrong size ends the export, file and folder.
    let path = try #require(
      await messenger.call(channel, "start", ["width": 32, "height": 32, "fps": 10]) as? String)
    #expect(code(await messenger.call(channel, "frame", frame(10))) == "bad")
    #expect(!FileManager.default.fileExists(atPath: (path as NSString).deletingLastPathComponent))
    #expect(code(await messenger.call(channel, "frame", frame(32 * 32 * 4))) == "none")
  }

  @Test func writesOnlyItsOwnFile() async throws {
    let messenger = FakeMessenger()
    ReplayWriter.register(with: messenger)
    let shares = FileManager.default.temporaryDirectory
      .appendingPathComponent("claktory-share", isDirectory: true).standardizedFileURL.path
    let path = try #require(
      await messenger.call(channel, "start", ["width": 32, "height": 32, "fps": 10]) as? String)
    let url = URL(fileURLWithPath: path)
    #expect(url.lastPathComponent == "claktory-replay.mp4")
    #expect(url.deletingLastPathComponent().deletingLastPathComponent().standardizedFileURL.path == shares)
    for _ in 0..<10 {
      #expect(await messenger.call(channel, "frame", frame(32 * 32 * 4)) == nil)
    }
    #expect(await messenger.call(channel, "finish") as? String == path)
    let asset = AVURLAsset(url: url)
    let duration = try await asset.load(.duration)
    #expect(abs(duration.seconds - 1) < 0.01)
    try FileManager.default.removeItem(at: url.deletingLastPathComponent())

    // Cancelled, nothing stays; a new start is a new folder.
    let again = try #require(
      await messenger.call(channel, "start", ["width": 32, "height": 32, "fps": 10]) as? String)
    #expect(again != path)
    #expect(await messenger.call(channel, "cancel") == nil)
    #expect(!FileManager.default.fileExists(atPath: (again as NSString).deletingLastPathComponent))
  }
}

struct VideoSoundTests {
  /// A video playing is heard with the ring switch on silent, as in
  /// Photos: the playback category while one plays, let go when none does.
  @MainActor @Test func aPlayingVideoTakesThePlaybackCategory() {
    let first = NSObject()
    let second = NSObject()
    VideoSound.playing(first, true)
    #expect(AVAudioSession.sharedInstance().category == .playback)
    #expect(AVAudioSession.sharedInstance().mode == .moviePlayback)
    VideoSound.playing(second, true)
    VideoSound.playing(first, false)
    #expect(VideoSound.active)
    VideoSound.playing(second, false)
    #expect(!VideoSound.active)
    VideoSound.playing(second, false)
    #expect(!VideoSound.active)
  }
}
