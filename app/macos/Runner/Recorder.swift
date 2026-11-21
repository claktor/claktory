// Records voice messages (app/lib/recorder.dart), on the Mac and the
// iPhone: the iPhone's Runner builds this same file. AAC in .m4a, 16 kHz
// mono, about 32 kbps, with AVAudioRecorder, into the path the app gives.
//
// factory/recorder: "access" (granted, denied, undetermined or
// restricted), "request" (asks the person; true if allowed), "openSettings"
// (the iPhone's Settings for this app; nothing on the Mac), "start" (a file
// path; errors "denied", "busy" or "failed"), "stop" ({path, durationMs}),
// "cancel" (stops and deletes the file).
// factory/recorder/events: while recording, about every 50 ms, {level 0…1,
// ms elapsed}; then once {ended: "limit" | "interrupted" | "error"} if it
// stops by itself (at ten minutes; a call or Siri on the iPhone; a
// failure). After limit or interrupted the file stays for "stop" to hand
// back; after error it's deleted.

import AVFoundation

#if os(macOS)
  import FlutterMacOS
#else
  import Flutter
  import UIKit
#endif

final class Recorder: NSObject, AVAudioRecorderDelegate, FlutterStreamHandler {
  /// Kept for the app's lifetime, once registered.
  private static var shared: Recorder?

  /// The longest recording, in seconds: it stops by itself then.
  static let maxSeconds: TimeInterval = 600

  static func register(with messenger: FlutterBinaryMessenger) {
    shared = Recorder(messenger: messenger)
  }

  private let channel: FlutterMethodChannel
  private let events: FlutterEventChannel
  private var sink: FlutterEventSink?
  private var recorder: AVAudioRecorder?
  private var ticker: Timer?
  /// How long the last recording ran, once it stopped by itself.
  private var stoppedAfter: TimeInterval?

  private init(messenger: FlutterBinaryMessenger) {
    channel = FlutterMethodChannel(name: "factory/recorder", binaryMessenger: messenger)
    events = FlutterEventChannel(name: "factory/recorder/events", binaryMessenger: messenger)
    super.init()
    channel.setMethodCallHandler { [weak self] call, result in
      self?.handle(call, result)
    }
    events.setStreamHandler(self)
    #if os(iOS)
      observeInterruptions()
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
    switch call.method {
    case "access":
      switch AVCaptureDevice.authorizationStatus(for: .audio) {
      case .authorized: result("granted")
      case .notDetermined: result("undetermined")
      case .restricted: result("restricted")
      default: result("denied")
      }
    case "request":
      AVCaptureDevice.requestAccess(for: .audio) { granted in
        DispatchQueue.main.async { result(granted) }
      }
    case "openSettings":
      #if os(iOS)
        if let url = URL(string: UIApplication.openSettingsURLString) {
          UIApplication.shared.open(url)
        }
      #endif
      result(nil)
    case "start":
      guard let path = call.arguments as? String else {
        return result(FlutterError(code: "failed", message: "No file to record into.", details: nil))
      }
      start(path, result)
    case "stop":
      guard let recorder else { return result(nil) }
      let seconds = stoppedAfter ?? recorder.currentTime
      // Let go first, so its delegate doesn't take this for stopping by
      // itself.
      finish()
      recorder.stop()
      result(["path": recorder.url.path, "durationMs": Int(seconds * 1000)])
    case "cancel":
      let cancelled = recorder
      finish()
      cancelled?.stop()
      cancelled?.deleteRecording()
      result(nil)
    default:
      result(FlutterMethodNotImplemented)
    }
  }

  private func start(_ path: String, _ result: @escaping FlutterResult) {
    if recorder != nil {
      return result(FlutterError(code: "busy", message: "Already recording.", details: nil))
    }
    guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else {
      return result(
        FlutterError(code: "denied", message: "Claktory can't use the microphone.", details: nil))
    }
    let settings: [String: Any] = [
      AVFormatIDKey: kAudioFormatMPEG4AAC,
      AVSampleRateKey: 16_000,
      AVNumberOfChannelsKey: 1,
      AVEncoderBitRateKey: 32_000,
    ]
    do {
      #if os(iOS)
        try prepareSession()
      #endif
      let url = URL(fileURLWithPath: path)
      try FileManager.default.createDirectory(
        at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
      let recorder = try AVAudioRecorder(url: url, settings: settings)
      recorder.delegate = self
      recorder.isMeteringEnabled = true
      guard recorder.record(forDuration: Self.maxSeconds) else {
        return result(
          FlutterError(code: "failed", message: "The microphone couldn't start.", details: nil))
      }
      self.recorder = recorder
      stoppedAfter = nil
      ticker = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self] _ in
        self?.tick()
      }
      result(nil)
    } catch {
      result(FlutterError(code: "failed", message: error.localizedDescription, details: nil))
    }
  }

  /// The input's loudness now, 0 to 1, and how long it's run.
  private func tick() {
    guard let recorder, recorder.isRecording else { return }
    recorder.updateMeters()
    // -50 dB and below is silence; 0 dB is as loud as it gets.
    let power = Double(recorder.averagePower(forChannel: 0))
    sink?(["level": max(0, min(1, (power + 50) / 50)), "ms": Int(recorder.currentTime * 1000)])
  }

  /// Done with the recording, whichever way.
  private func finish() {
    ticker?.invalidate()
    ticker = nil
    recorder = nil
    stoppedAfter = nil
    #if os(iOS)
      endSession()
    #endif
  }

  /// Stopped by itself: at ten minutes, or the microphone failed.
  func audioRecorderDidFinishRecording(_ recorder: AVAudioRecorder, successfully flag: Bool) {
    guard recorder === self.recorder, stoppedAfter == nil else { return }
    ticker?.invalidate()
    ticker = nil
    if flag {
      stoppedAfter = Self.maxSeconds
      sink?(["ended": "limit"])
    } else {
      finish()
      recorder.deleteRecording()
      sink?(["ended": "error"])
    }
  }

  func audioRecorderEncodeErrorDidOccur(_ recorder: AVAudioRecorder, error: Error?) {
    guard recorder === self.recorder else { return }
    finish()
    recorder.stop()
    recorder.deleteRecording()
    sink?(["ended": "error"])
  }

  #if os(iOS)
    // The iPhone's audio session (Mobile's, slice 5): recording needs the
    // record category while it runs, and a call or Siri interrupts it.

    /// Before recording: the session in a category that records.
    private func prepareSession() throws {
      let session = AVAudioSession.sharedInstance()
      // AirPods and other headsets record too; what plays after comes from
      // the speaker, not the earpiece.
      try session.setCategory(
        .playAndRecord, mode: .default, options: [.defaultToSpeaker, .allowBluetooth])
      try session.setActive(true)
    }

    /// After recording: the session goes back, other apps' audio resumes.
    private func endSession() {
      try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    private func observeInterruptions() {
      NotificationCenter.default.addObserver(
        forName: AVAudioSession.interruptionNotification, object: nil, queue: .main
      ) { [weak self] note in
        guard let self, let recorder = self.recorder, self.stoppedAfter == nil,
          let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
          AVAudioSession.InterruptionType(rawValue: raw) == .began
        else { return }
        self.stoppedAfter = recorder.currentTime
        self.ticker?.invalidate()
        self.ticker = nil
        recorder.pause()
        self.sink?(["ended": "interrupted"])
      }
    }
  #endif
}
