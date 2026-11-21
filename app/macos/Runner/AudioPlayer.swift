// Plays voice messages back (app/lib/audio.dart), on the Mac and the
// iPhone: the iPhone's Runner builds this same file. One at a time, from a
// file on this device, with AVAudioPlayer.
//
// factory/audio: "play" ({path, fromMs}: starts that file, or resumes it
// from fromMs; whatever played before stops), "pause", "seek" (ms), "stop".
// "play" fails with "failed" when the file can't be played.
// factory/audio/events: while one plays, about every 100 ms, and on every
// change: {path, positionMs, durationMs, state: "playing" | "paused" |
// "ended" | "stopped"}.

import AVFoundation

#if os(macOS)
  import FlutterMacOS
#else
  import Flutter
#endif

final class AudioPlayer: NSObject, AVAudioPlayerDelegate, FlutterStreamHandler {
  /// Kept for the app's lifetime, once registered.
  private static var shared: AudioPlayer?

  static func register(with messenger: FlutterBinaryMessenger) {
    shared = AudioPlayer(messenger: messenger)
  }

  private let channel: FlutterMethodChannel
  private let events: FlutterEventChannel
  private var sink: FlutterEventSink?
  private var player: AVAudioPlayer?
  private var path: String?
  private var ticker: Timer?

  private init(messenger: FlutterBinaryMessenger) {
    channel = FlutterMethodChannel(name: "factory/audio", binaryMessenger: messenger)
    events = FlutterEventChannel(name: "factory/audio/events", binaryMessenger: messenger)
    super.init()
    channel.setMethodCallHandler { [weak self] call, result in
      self?.handle(call, result)
    }
    events.setStreamHandler(self)
    #if os(iOS)
      observeSession()
    #endif
  }

  func onListen(withArguments arguments: Any?, eventSink: @escaping FlutterEventSink)
    -> FlutterError?
  {
    sink = eventSink
    return nil
  }

  func onCancel(withArguments arguments: Any?) -> FlutterError? {
    sink = nil
    return nil
  }

  private func handle(_ call: FlutterMethodCall, _ result: @escaping FlutterResult) {
    let arguments = call.arguments as? [String: Any]
    switch call.method {
    case "play":
      guard let path = arguments?["path"] as? String else {
        return result(FlutterError(code: "failed", message: "No file to play.", details: nil))
      }
      let from = (arguments?["fromMs"] as? NSNumber)?.doubleValue ?? 0
      play(path, from: from / 1000, result)
    case "pause":
      player?.pause()
      stopTicking()
      report("paused")
      result(nil)
    case "seek":
      if let player, let ms = (call.arguments as? NSNumber)?.doubleValue {
        player.currentTime = max(0, min(player.duration, ms / 1000))
        report(player.isPlaying ? "playing" : "paused")
      }
      result(nil)
    case "stop":
      stop()
      result(nil)
    default:
      result(FlutterMethodNotImplemented)
    }
  }

  private func play(_ path: String, from seconds: TimeInterval, _ result: @escaping FlutterResult) {
    if path != self.path || player == nil {
      stop()
      do {
        #if os(iOS)
          try prepareSession()
        #endif
        let player = try AVAudioPlayer(contentsOf: URL(fileURLWithPath: path))
        player.delegate = self
        player.prepareToPlay()
        self.player = player
        self.path = path
      } catch {
        return result(
          FlutterError(code: "failed", message: "This recording can't be played.", details: nil))
      }
    }
    guard let player else { return result(nil) }
    player.currentTime = max(0, min(player.duration, seconds))
    guard player.play() else {
      return result(
        FlutterError(code: "failed", message: "This recording can't be played.", details: nil))
    }
    ticker?.invalidate()
    ticker = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
      self?.report("playing")
    }
    report("playing")
    result(nil)
  }

  /// Done with what played, telling the app.
  private func stop() {
    guard let player else { return }
    player.stop()
    stopTicking()
    report("stopped")
    self.player = nil
    path = nil
    #if os(iOS)
      endSession()
    #endif
  }

  private func stopTicking() {
    ticker?.invalidate()
    ticker = nil
  }

  private func report(_ state: String) {
    guard let player, let path else { return }
    sink?([
      "path": path,
      "positionMs": Int(player.currentTime * 1000),
      "durationMs": Int(player.duration * 1000),
      "state": state,
    ])
  }

  func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
    guard player === self.player, let path else { return }
    stopTicking()
    sink?([
      "path": path,
      "positionMs": Int(player.duration * 1000),
      "durationMs": Int(player.duration * 1000),
      "state": "ended",
    ])
    self.player = nil
    self.path = nil
    #if os(iOS)
      endSession()
    #endif
  }

  #if os(iOS)
    // The iPhone's audio session (Mobile's, slice 5): playback plays even
    // with the ring switch on silent, and lets other apps' audio resume
    // after.

    private func prepareSession() throws {
      let session = AVAudioSession.sharedInstance()
      try session.setCategory(.playback, mode: .spokenAudio)
      try session.setActive(true)
    }

    private func endSession() {
      try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    /// A call or Siri pauses what plays, and so does unplugging headphones
    /// (or AirPods leaving), as everywhere on the iPhone: rather than go on
    /// out loud. The app hears it paused, to play on from there.
    private func observeSession() {
      let center = NotificationCenter.default
      center.addObserver(
        forName: AVAudioSession.interruptionNotification, object: nil, queue: .main
      ) { [weak self] note in
        guard let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
          AVAudioSession.InterruptionType(rawValue: raw) == .began
        else { return }
        self?.pauseBySystem()
      }
      center.addObserver(
        forName: AVAudioSession.routeChangeNotification, object: nil, queue: .main
      ) { [weak self] note in
        guard let raw = note.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt,
          AVAudioSession.RouteChangeReason(rawValue: raw) == .oldDeviceUnavailable
        else { return }
        self?.pauseBySystem()
      }
    }

    private func pauseBySystem() {
      guard let player, player.isPlaying || ticker != nil else { return }
      player.pause()
      stopTicking()
      report("paused")
    }
  #endif
}
