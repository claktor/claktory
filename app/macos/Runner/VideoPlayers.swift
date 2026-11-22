// The system's video player (AVPlayer) for the app's video viewer
// (app/lib/viewers/video.dart), on the Mac and the iPhone: the iPhone's
// Runner builds this same file. Each video is a Flutter texture its frames
// are drawn into; the app sends "open" (a file on this device), "play",
// "pause", "seek" and "close" on the claktory/video channel, and gets each
// player's "state" back on it whenever it plays, pauses, moves or ends.

import AVFoundation

#if os(macOS)
  import FlutterMacOS
#else
  import Flutter
#endif

final class VideoPlayers: NSObject {
  /// Kept for the app's lifetime, once registered.
  private static var shared: VideoPlayers?

  static func register(with registrar: FlutterPluginRegistrar) {
    #if os(macOS)
      let players = VideoPlayers(textures: registrar.textures, messenger: registrar.messenger)
    #else
      let players = VideoPlayers(textures: registrar.textures(), messenger: registrar.messenger())
    #endif
    shared = players
  }

  private let textures: FlutterTextureRegistry
  private let channel: FlutterMethodChannel
  private var players: [Int64: VideoPlayer] = [:]

  private init(textures: FlutterTextureRegistry, messenger: FlutterBinaryMessenger) {
    self.textures = textures
    channel = FlutterMethodChannel(name: "claktory/video", binaryMessenger: messenger)
    super.init()
    channel.setMethodCallHandler { [weak self] call, result in
      self?.handle(call, result)
    }
  }

  private func handle(_ call: FlutterMethodCall, _ result: @escaping FlutterResult) {
    let arguments = call.arguments as? [String: Any] ?? [:]
    if call.method == "open" {
      guard let path = arguments["path"] as? String else {
        return result(FlutterError(code: "arguments", message: "No file to play.", details: nil))
      }
      return open(path, result)
    }
    guard let texture = (arguments["texture"] as? NSNumber)?.int64Value,
      let player = players[texture]
    else { return result(nil) }
    switch call.method {
    case "play": player.play()
    case "pause": player.pause()
    case "seek":
      return player.seek(to: (arguments["seconds"] as? NSNumber)?.doubleValue ?? 0) {
        result(nil)
      }
    case "close":
      player.close()
      players[texture] = nil
      textures.unregisterTexture(texture)
    default: return result(FlutterMethodNotImplemented)
    }
    result(nil)
  }

  /// Answers the new player's texture once the file is ready to play, or
  /// an error saying why it can't be.
  private func open(_ path: String, _ result: @escaping FlutterResult) {
    let player = VideoPlayer(url: URL(fileURLWithPath: path))
    let texture = textures.register(player)
    player.texture = texture
    player.frameAvailable = { [weak self] in self?.textures.textureFrameAvailable(texture) }
    player.changed = { [weak self] state in
      var state = state
      state["texture"] = texture
      self?.channel.invokeMethod("state", arguments: state)
    }
    players[texture] = player
    player.load { [weak self] error in
      if let error {
        player.close()
        self?.players[texture] = nil
        self?.textures.unregisterTexture(texture)
        result(FlutterError(code: "failed", message: error, details: nil))
      } else {
        var answer = player.state()
        answer["texture"] = texture
        result(answer)
      }
    }
  }
}

/// One video: an AVPlayer whose frames are copied out for its texture.
final class VideoPlayer: NSObject, FlutterTexture {
  var texture: Int64 = 0
  var frameAvailable: (() -> Void)?
  var changed: (([String: Any]) -> Void)?

  /// The file's own asset, sound and all.
  private let asset: AVURLAsset
  /// What plays: the picture alone until the first play, if it has sound.
  private var item: AVPlayerItem
  private let player: AVPlayer
  private let output = AVPlayerItemVideoOutput(pixelBufferAttributes: [
    kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
    kCVPixelBufferMetalCompatibilityKey as String: true,
    kCVPixelBufferIOSurfacePropertiesKey as String: [String: Any](),
  ])
  private var observations: [NSKeyValueObservation] = []
  private var timeObserver: Any?
  private var ticker: Timer?
  /// The frame Flutter draws next, swapped in on the main thread and read
  /// on Flutter's raster thread.
  private var frame: CVPixelBuffer?
  private let frameLock = NSLock()
  /// Quarter turns clockwise the frame is shown at (a phone held upright).
  private var quarterTurns = 0
  /// The frame's size in pixels, before it's turned.
  private var size = CGSize.zero
  /// Whether its sound is still left out: only the picture plays until the
  /// first play. An item with sound opens the sound output as it loads,
  /// and where that output is also a microphone (a USB headset, a dock, a
  /// virtual machine's one sound device) macOS takes that for using the
  /// microphone and asks for it, while the video only loads (Abbacchio's
  /// rows 492 and 495: turning its sound tracks off as they come was too
  /// late). Left out until then, a prompt there comes as its sound starts.
  private var soundLeftOut = false
  /// Where it was when its sound went in, until the whole file is there.
  private var swappingAt: CMTime?

  /// Whether what plays now has sound, for the check.
  var soundEnabled: Bool { !item.asset.tracks(withMediaType: .audio).isEmpty }

  init(url: URL) {
    asset = AVURLAsset(url: url)
    // A file on this device: its tracks are read at once.
    let picture = asset.tracks(withMediaType: .audio).isEmpty ? nil : Self.picture(of: asset)
    soundLeftOut = picture != nil
    item = AVPlayerItem(asset: picture ?? asset)
    player = AVPlayer(playerItem: item)
    player.actionAtItemEnd = .pause
    super.init()
    item.add(output)
  }

  /// [asset]'s picture without its sound; nil if it can't be made.
  private static func picture(of asset: AVAsset) -> AVAsset? {
    let videos = asset.tracks(withMediaType: .video)
    guard !videos.isEmpty else { return nil }
    let composition = AVMutableComposition()
    let range = CMTimeRange(start: .zero, duration: asset.duration)
    for video in videos {
      guard
        let track = composition.addMutableTrack(
          withMediaType: .video, preferredTrackID: kCMPersistentTrackID_Invalid),
        (try? track.insertTimeRange(range, of: video, at: .zero)) != nil
      else { return nil }
      track.preferredTransform = video.preferredTransform
    }
    return composition
  }

  /// Puts the sound in: the whole file plays from where the picture was,
  /// then [then].
  private func addSound(then: @escaping () -> Void) {
    soundLeftOut = false
    let at = player.currentTime()
    // Meanwhile (macOS may be asking for the microphone) it still says
    // where it was, not the new item's start.
    swappingAt = at
    let whole = AVPlayerItem(asset: asset)
    item.remove(output)
    whole.add(output)
    item = whole
    player.replaceCurrentItem(with: whole)
    var done = false
    observations.append(
      whole.observe(\.status, options: [.initial, .new]) { [weak self] whole, _ in
        DispatchQueue.main.async {
          guard let self, !done, whole.status != .unknown else { return }
          done = true
          self.player.seek(to: at, toleranceBefore: .zero, toleranceAfter: .zero) { _ in
            DispatchQueue.main.async {
              self.swappingAt = nil
              then()
            }
          }
        }
      })
  }

  /// Calls back once with nil when ready to play, or with why it can't be.
  func load(_ done: @escaping (String?) -> Void) {
    var answered = false
    let loading = item
    observations.append(
      loading.observe(\.status, options: [.initial, .new]) { [weak self] item, _ in
        DispatchQueue.main.async {
          guard let self, !answered else { return }
          switch item.status {
          case .readyToPlay:
            answered = true
            self.ready()
            done(nil)
          case .failed where self.soundLeftOut && item === self.item:
            // Its picture alone wouldn't play here: the whole file, sound
            // and all, as before (a duplex output may ask for the
            // microphone now).
            answered = true
            NSLog(
              "Claktory video: its picture alone can't play (%@); playing it whole",
              String(describing: item.error))
            self.soundLeftOut = false
            let whole = AVPlayerItem(asset: self.asset)
            item.remove(self.output)
            whole.add(self.output)
            self.item = whole
            self.player.replaceCurrentItem(with: whole)
            self.load(done)
          case .failed:
            answered = true
            done(item.error?.localizedDescription ?? "Claktory can't play this video.")
          default: break
          }
        }
      })
  }

  private func ready() {
    size = item.presentationSize
    if let track = asset.tracks(withMediaType: .video).first {
      size = track.naturalSize
      let transform = track.preferredTransform
      let turns = Int((atan2(transform.b, transform.a) * 2 / .pi).rounded())
      quarterTurns = (turns % 4 + 4) % 4
    }
    observations.append(
      player.observe(\.timeControlStatus, options: [.new]) { [weak self] _, _ in
        DispatchQueue.main.async { self?.report() }
      })
    timeObserver = player.addPeriodicTimeObserver(
      forInterval: CMTime(value: 1, timescale: 10), queue: .main
    ) { [weak self] _ in self?.report() }
    ticker = Timer(timeInterval: 1.0 / 60, repeats: true) { [weak self] _ in self?.tick() }
    RunLoop.main.add(ticker!, forMode: .common)
    // The first frame, before anything plays.
    seek(to: 0) {}
  }

  func play() {
    if soundLeftOut { return addSound { [weak self] in self?.play() } }
    // From the start again once it's played to the end.
    if let duration = seconds(item.duration), seconds(player.currentTime()) ?? 0 >= duration {
      player.seek(to: .zero)
    }
    player.play()
  }

  func pause() {
    player.pause()
  }

  func seek(to seconds: Double, _ done: @escaping () -> Void) {
    let time = CMTime(seconds: max(0, seconds), preferredTimescale: 600)
    player.seek(to: time, toleranceBefore: .zero, toleranceAfter: .zero) { [weak self] _ in
      DispatchQueue.main.async {
        self?.report()
        done()
      }
    }
  }

  func close() {
    ticker?.invalidate()
    ticker = nil
    if let timeObserver { player.removeTimeObserver(timeObserver) }
    timeObserver = nil
    observations.forEach { $0.invalidate() }
    observations = []
    player.pause()
    player.replaceCurrentItem(with: nil)
    frameAvailable = nil
    changed = nil
  }

  /// Takes the newest decoded frame, if there's one Flutter hasn't drawn.
  private func tick() {
    let time = output.itemTime(forHostTime: CACurrentMediaTime())
    guard output.hasNewPixelBuffer(forItemTime: time),
      let buffer = output.copyPixelBuffer(forItemTime: time, itemTimeForDisplay: nil)
    else { return }
    frameLock.lock()
    frame = buffer
    frameLock.unlock()
    frameAvailable?()
  }

  func copyPixelBuffer() -> Unmanaged<CVPixelBuffer>? {
    frameLock.lock()
    defer { frameLock.unlock() }
    return frame.map { Unmanaged.passRetained($0) }
  }

  private func report() {
    changed?(state())
  }

  /// What the app's viewer shows: whether it's playing, where (seconds), how
  /// long it is, and the frame's size and turn.
  func state() -> [String: Any] {
    [
      "playing": player.timeControlStatus != .paused,
      "position": seconds(swappingAt ?? player.currentTime()) ?? 0,
      "duration": seconds(item.duration) ?? 0,
      "width": Double(size.width),
      "height": Double(size.height),
      "quarterTurns": quarterTurns,
    ]
  }

  private func seconds(_ time: CMTime) -> Double? {
    let value = CMTimeGetSeconds(time)
    return value.isFinite ? value : nil
  }
}
