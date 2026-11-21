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
