// History's video export (app/lib/history/export.dart, #458 E1): encodes
// the frames the app renders, one at a time, into an MP4 with
// AVAssetWriter, on the Mac and the iPhone (the iPhone's Runner builds this
// same file).
//
// claktory/replay-writer:
// - "start" {width, height, fps}: a new MP4, made here, in a fresh folder
//   of the app's temporary folder's claktory-share (where
//   lib/share_files.dart keeps what's shared and sweeps it). Answers its
//   path. It takes no path, so it can't be pointed at other files. One at
//   a time: a new start cancels the one before.
// - "frame" {rgba: the frame's pixels, width × height × 4 bytes, RGBA}:
//   appended as the next frame, at 1/fps after the last. Answers once
//   it's in, so the app sends the next only then.
// - "finish": the file, complete. Answers its path.
// - "cancel": stops and deletes it, folder and all.
// Errors: "bad" (arguments), "limit" (too big or too long), "none" (no
// export started), "failed" (the encoder's).
//
// The file carries no metadata: no location, device or software, nothing
// AVAssetWriter adds of its own (Fugo); its header's dates are zeroed by
// the app when dates aren't shown.
import AVFoundation
import Accelerate

// Without Flutter (RunnerTests/ReplayWriterCheck.swift), the encoder alone.
#if canImport(FlutterMacOS)
  import FlutterMacOS
#elseif canImport(Flutter)
  import Flutter
#endif

/// The encoder alone, without Flutter, so it can be checked by itself.
final class ReplayEncoder {
  /// The largest frame side, and the most frames (a minute and a half at
  /// 60 fps): a bad caller can't fill the disk.
  static let maxSide = 1920
  static let maxFrames = 5400

  enum Failure: Error {
    case bad(String)
    case limit(String)
    case failed(String)
  }

  /// Where exports are made: a fresh folder in the share folder of the
  /// app's temporary folder, which lib/share_files.dart sweeps.
  static func newFile() throws -> URL {
    let folder = FileManager.default.temporaryDirectory
      .appendingPathComponent("claktory-share", isDirectory: true)
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    return folder.appendingPathComponent("claktory-replay.mp4")
  }

  let url: URL
  let width: Int
  let height: Int
  let fps: Int32
  private(set) var frames = 0
  private let writer: AVAssetWriter
  private let input: AVAssetWriterInput
  private let adaptor: AVAssetWriterInputPixelBufferAdaptor

  init(url: URL, width: Int, height: Int, fps: Int) throws {
    guard width >= 16, height >= 16, width % 2 == 0, height % 2 == 0 else {
      throw Failure.bad("width and height: even, 16 or more")
    }
    guard width <= Self.maxSide, height <= Self.maxSide else {
      throw Failure.limit("at most \(Self.maxSide) a side")
    }
    guard fps >= 1, fps <= 60 else { throw Failure.bad("fps: 1 to 60") }
    self.url = url
    self.width = width
    self.height = height
    self.fps = Int32(fps)
    do {
      writer = try AVAssetWriter(outputURL: url, fileType: .mp4)
    } catch {
      throw Failure.failed(error.localizedDescription)
    }
    // Nothing about where or on what it was made.
    writer.metadata = []
    writer.shouldOptimizeForNetworkUse = true
    let bits = Double(width * height * fps) * 0.15
    input = AVAssetWriterInput(
      mediaType: .video,
      outputSettings: [
        AVVideoCodecKey: AVVideoCodecType.h264,
        AVVideoWidthKey: width,
        AVVideoHeightKey: height,
        AVVideoColorPropertiesKey: [
          AVVideoColorPrimariesKey: AVVideoColorPrimaries_ITU_R_709_2,
          AVVideoTransferFunctionKey: AVVideoTransferFunction_ITU_R_709_2,
          AVVideoYCbCrMatrixKey: AVVideoYCbCrMatrix_ITU_R_709_2,
        ],
        AVVideoCompressionPropertiesKey: [
          AVVideoAverageBitRateKey: Int(bits),
          AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
          AVVideoMaxKeyFrameIntervalKey: fps * 2,
        ],
      ])
    input.expectsMediaDataInRealTime = false
    input.metadata = []
    adaptor = AVAssetWriterInputPixelBufferAdaptor(
      assetWriterInput: input,
      sourcePixelBufferAttributes: [
        kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
        kCVPixelBufferWidthKey as String: width,
        kCVPixelBufferHeightKey as String: height,
      ])
    guard writer.canAdd(input) else { throw Failure.failed("can't add the video") }
    writer.add(input)
    guard writer.startWriting() else {
      throw Failure.failed(writer.error?.localizedDescription ?? "can't start")
    }
    writer.startSession(atSourceTime: .zero)
  }

  /// Appends [rgba] (width × height × 4 bytes) as the next frame, waiting
  /// for the encoder to take it. Called on one queue, never the main one.
  func append(rgba: Data) throws {
    guard rgba.count == width * height * 4 else {
      throw Failure.bad("a frame is \(width * height * 4) bytes")
    }
    guard frames < Self.maxFrames else {
      throw Failure.limit("at most \(Self.maxFrames) frames")
    }
    // The encoder takes frames as fast as it can; up to 10 s for one.
    var waited = 0
    while !input.isReadyForMoreMediaData {
      if writer.status != .writing || waited > 5000 {
        throw Failure.failed(writer.error?.localizedDescription ?? "the encoder stopped")
      }
      usleep(2000)
      waited += 1
    }
    guard let pool = adaptor.pixelBufferPool else {
      throw Failure.failed("no pixel buffers")
    }
    var made: CVPixelBuffer?
    guard CVPixelBufferPoolCreatePixelBuffer(nil, pool, &made) == kCVReturnSuccess,
      let buffer = made
    else { throw Failure.failed("no pixel buffer") }
    CVPixelBufferLockBaseAddress(buffer, [])
    defer { CVPixelBufferUnlockBaseAddress(buffer, []) }
    guard let base = CVPixelBufferGetBaseAddress(buffer) else {
      throw Failure.failed("no pixel buffer memory")
    }
    let error: vImage_Error = rgba.withUnsafeBytes { raw in
      var source = vImage_Buffer(
        data: UnsafeMutableRawPointer(mutating: raw.baseAddress!),
        height: vImagePixelCount(height), width: vImagePixelCount(width),
        rowBytes: width * 4)
      var target = vImage_Buffer(
        data: base, height: vImagePixelCount(height), width: vImagePixelCount(width),
        rowBytes: CVPixelBufferGetBytesPerRow(buffer))
      // RGBA to BGRA.
      let map: [UInt8] = [2, 1, 0, 3]
      return vImagePermuteChannels_ARGB8888(&source, &target, map, vImage_Flags(kvImageNoFlags))
    }
    guard error == kvImageNoError else { throw Failure.failed("vImage \(error)") }
    // Tagged as the output is, so the conversion to YUV doesn't guess and
    // wash the colours out (Gelato).
    CVBufferSetAttachment(
      buffer, kCVImageBufferColorPrimariesKey, kCVImageBufferColorPrimaries_ITU_R_709_2,
      .shouldPropagate)
    CVBufferSetAttachment(
      buffer, kCVImageBufferTransferFunctionKey, kCVImageBufferTransferFunction_ITU_R_709_2,
      .shouldPropagate)
    CVBufferSetAttachment(
      buffer, kCVImageBufferYCbCrMatrixKey, kCVImageBufferYCbCrMatrix_ITU_R_709_2,
      .shouldPropagate)
    let at = CMTime(value: CMTimeValue(frames), timescale: fps)
    guard adaptor.append(buffer, withPresentationTime: at) else {
      throw Failure.failed(writer.error?.localizedDescription ?? "a frame wasn't taken")
    }
    frames += 1
  }

  /// Ends the file; [done] gets nil once it's complete.
  func finish(_ done: @escaping (Error?) -> Void) {
    guard frames > 0 else {
      cancel()
      done(Failure.bad("no frames"))
      return
    }
    input.markAsFinished()
    writer.endSession(atSourceTime: CMTime(value: CMTimeValue(frames), timescale: fps))
    writer.finishWriting { [writer] in
      if writer.status == .completed {
        done(nil)
      } else {
        done(Failure.failed(writer.error?.localizedDescription ?? "couldn't finish"))
      }
    }
  }

  /// Stops, and deletes what was written, its folder too.
  func cancel() {
    if writer.status == .writing { writer.cancelWriting() }
    try? FileManager.default.removeItem(at: url.deletingLastPathComponent())
  }
}

#if canImport(FlutterMacOS) || canImport(Flutter)
final class ReplayWriter {
  /// Kept for the app's lifetime, once registered.
  private static var shared: ReplayWriter?

  static func register(with messenger: FlutterBinaryMessenger) {
    shared = ReplayWriter(messenger: messenger)
  }

  private let channel: FlutterMethodChannel
  private let queue = DispatchQueue(label: "claktory.replay-writer")

  /// The export under way; touched only on [queue].
  private var encoder: ReplayEncoder?

  init(messenger: FlutterBinaryMessenger) {
    channel = FlutterMethodChannel(name: "claktory/replay-writer", binaryMessenger: messenger)
    channel.setMethodCallHandler { [weak self] call, result in
      self?.handle(call, result)
    }
  }

  private func handle(_ call: FlutterMethodCall, _ result: @escaping FlutterResult) {
    let arguments = call.arguments as? [String: Any]
    // Answers on the main thread, as Flutter wants.
    let reply: (Any?) -> Void = { value in DispatchQueue.main.async { result(value) } }
    switch call.method {
    case "start":
      guard let width = arguments?["width"] as? Int,
        let height = arguments?["height"] as? Int,
        let fps = arguments?["fps"] as? Int
      else {
        result(FlutterError(code: "bad", message: "width, height and fps", details: nil))
        return
      }
      queue.async {
        self.encoder?.cancel()
        self.encoder = nil
        do {
          let url = try ReplayEncoder.newFile()
          do {
            self.encoder = try ReplayEncoder(url: url, width: width, height: height, fps: fps)
          } catch {
            try? FileManager.default.removeItem(at: url.deletingLastPathComponent())
            throw error
          }
          reply(url.path)
        } catch {
          reply(Self.flutterError(error))
        }
      }
    case "frame":
      guard let bytes = arguments?["rgba"] as? FlutterStandardTypedData else {
        result(FlutterError(code: "bad", message: "rgba", details: nil))
        return
      }
      queue.async {
        guard let encoder = self.encoder else {
          reply(FlutterError(code: "none", message: "no export started", details: nil))
          return
        }
        do {
          try encoder.append(rgba: bytes.data)
          reply(nil)
        } catch {
          // A frame that fails ends it: nothing half made stays.
          encoder.cancel()
          self.encoder = nil
          reply(Self.flutterError(error))
        }
      }
    case "finish":
      queue.async {
        guard let encoder = self.encoder else {
          reply(FlutterError(code: "none", message: "no export started", details: nil))
          return
        }
        self.encoder = nil
        encoder.finish { error in
          if let error = error {
            encoder.cancel()
            reply(Self.flutterError(error))
          } else {
            reply(encoder.url.path)
          }
        }
      }
    case "cancel":
      queue.async {
        self.encoder?.cancel()
        self.encoder = nil
        reply(nil)
      }
    default:
      result(FlutterMethodNotImplemented)
    }
  }

  private static func flutterError(_ error: Error) -> FlutterError {
    switch error {
    case ReplayEncoder.Failure.bad(let why):
      return FlutterError(code: "bad", message: why, details: nil)
    case ReplayEncoder.Failure.limit(let why):
      return FlutterError(code: "limit", message: why, details: nil)
    case ReplayEncoder.Failure.failed(let why):
      return FlutterError(code: "failed", message: why, details: nil)
    default:
      return FlutterError(code: "failed", message: error.localizedDescription, details: nil)
    }
  }
}
#endif
