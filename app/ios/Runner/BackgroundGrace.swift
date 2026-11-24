import Flutter
import UIKit

/// A short grace after leaving the app (#413, iphone-battery.md §3): on
/// going to the background the app asks iOS for time to keep running, so
/// its connections stay alive through a quick trip to another app (Messages
/// and back), and coming back within it only checks them rather than
/// making them afresh (lib/background_grace.dart).
///
/// `factory/device/grace` tells Dart `began` once iOS gives the time, and
/// `ended` once it's over: after [length], when iOS says time is up, or at
/// once when iOS gives none. Coming back ends it quietly, so Dart still
/// reads it as held when it hears the app is back. Each is sent before the
/// time is handed back, while Dart still runs: what's sent then reaches it,
/// in order, before it hears the app is back.
final class BackgroundGrace: NSObject {
  static let shared = BackgroundGrace()

  /// How long the grace lasts at most; iOS gives about 30 s.
  static let length: TimeInterval = 25

  private var channel: FlutterMethodChannel?
  private var task: UIBackgroundTaskIdentifier = .invalid
  private var timer: Timer?

  static func register(with messenger: FlutterBinaryMessenger) {
    shared.channel = FlutterMethodChannel(name: "factory/device/grace", binaryMessenger: messenger)
    let center = NotificationCenter.default
    center.addObserver(
      shared, selector: #selector(left),
      name: UIApplication.didEnterBackgroundNotification, object: nil)
    center.addObserver(
      shared, selector: #selector(back),
      name: UIApplication.willEnterForegroundNotification, object: nil)
  }

  @objc private func left() {
    finish()
    task = UIApplication.shared.beginBackgroundTask(withName: "Claktory grace") { [weak self] in
      self?.over()
    }
    guard task != .invalid else {
      channel?.invokeMethod("ended", arguments: nil)
      return
    }
    channel?.invokeMethod("began", arguments: Self.length)
    timer = Timer.scheduledTimer(withTimeInterval: Self.length, repeats: false) { [weak self] _ in
      self?.over()
    }
  }

  /// Back in front: the grace isn't needed any more, and Dart, which reads
  /// it as held, checks its connections instead of making them afresh.
  @objc private func back() {
    finish()
  }

  /// Time's up: Dart hears it first, then the time goes back to iOS.
  private func over() {
    guard task != .invalid else { return }
    channel?.invokeMethod("ended", arguments: nil)
    finish()
  }

  private func finish() {
    timer?.invalidate()
    timer = nil
    guard task != .invalid else { return }
    UIApplication.shared.endBackgroundTask(task)
    task = .invalid
  }
}
