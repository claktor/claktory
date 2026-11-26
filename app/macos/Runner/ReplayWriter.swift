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
// On an iPhone or iPad, where a finished video is saved (a Mac's app moves
// it into ~/Movies/Claktory itself):
// - "saveToPhotos" {path}: adds it to Photos (add-only access, asked for
//   the first time). Errors "denied" when Photos isn't allowed.
// - "saveToFiles" {path, name}: the Files sheet to export a copy, named
//   `name`. Answers true once saved, false if the person cancelled.
// Both take only a video made here (in claktory-share), never another file.
// Errors: "bad" (arguments), "limit" (too big or too long), "none" (no
// export started), "failed" (the encoder's).
//
// The file carries no metadata: no location, device or software, nothing
// AVAssetWriter adds of its own (Fugo); its header's dates are zeroed by
// the app when dates aren't shown.
import AVFoundation
import Accelerate

#if os(iOS)
  import Photos
  import UIKit
#endif

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
    #if os(iOS)
      case "saveToPhotos":
        guard let url = Self.madeHere(arguments?["path"]) else {
          result(FlutterError(code: "bad", message: "a video made here", details: nil))
          return
        }
        Self.saveToPhotos(url, reply)
      case "saveToFiles":
        guard let url = Self.madeHere(arguments?["path"]),
          let name = arguments?["name"] as? String,
          Self.plainName(name)
        else {
          result(FlutterError(code: "bad", message: "a video made here, and a name", details: nil))
          return
        }
        SaveToFiles.show(url, named: name, result)
    #endif
    default:
      result(FlutterMethodNotImplemented)
    }
  }

  #if os(iOS)
    /// [path] when it's a video this writer made: an MP4 in a folder of
    /// claktory-share, nowhere else.
    private static func madeHere(_ path: Any?) -> URL? {
      guard let path = path as? String else { return nil }
      let shares = FileManager.default.temporaryDirectory
        .appendingPathComponent("claktory-share", isDirectory: true)
        .resolvingSymlinksInPath().standardizedFileURL.pathComponents
      let url = URL(fileURLWithPath: path).resolvingSymlinksInPath().standardizedFileURL
      let parts = url.pathComponents
      guard parts.count == shares.count + 2,
        Array(parts.prefix(shares.count)) == shares,
        url.pathExtension == "mp4",
        FileManager.default.fileExists(atPath: url.path)
      else { return nil }
      return url
    }

    /// A file name without folders in it.
    private static func plainName(_ name: String) -> Bool {
      !name.isEmpty && name.count <= 200 && !name.contains("/") && !name.hasPrefix(".")
    }

    private static func saveToPhotos(_ url: URL, _ reply: @escaping (Any?) -> Void) {
      PHPhotoLibrary.requestAuthorization(for: .addOnly) { status in
        guard status == .authorized || status == .limited else {
          reply(FlutterError(code: "denied", message: "Photos isn't allowed", details: nil))
          return
        }
        PHPhotoLibrary.shared().performChanges({
          let options = PHAssetResourceCreationOptions()
          options.originalFilename = url.lastPathComponent
          PHAssetCreationRequest.forAsset().addResource(with: .video, fileURL: url, options: options)
        }) { saved, error in
          if saved {
            reply(nil)
          } else {
            reply(
              FlutterError(
                code: "failed", message: error?.localizedDescription ?? "not saved", details: nil))
          }
        }
      }
    }
  #endif

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

#if os(iOS) && canImport(Flutter)
  /// The Files sheet exporting a copy of a video, under the name the app
  /// gives it. Answers true once it's saved, false if cancelled.
  final class SaveToFiles: NSObject, UIDocumentPickerDelegate {
    /// The sheet showing now, kept until it answers.
    private static var current: SaveToFiles?
    private var result: FlutterResult?
    private var copy: URL?

    static func show(_ url: URL, named name: String, _ result: @escaping FlutterResult) {
      let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
      let scene = scenes.first { $0.activationState == .foregroundActive } ?? scenes.first
      guard let window = scene?.keyWindow ?? scene?.windows.first,
        var top = window.rootViewController
      else {
        result(
          FlutterError(code: "unavailable", message: "Claktory's window isn't open", details: nil))
        return
      }
      while let shown = top.presentedViewController { top = shown }
      // A copy under its name, beside it: the sheet names what it saves
      // after the file.
      let named = url.deletingLastPathComponent().appendingPathComponent(name)
      do {
        if named != url {
          try? FileManager.default.removeItem(at: named)
          try FileManager.default.linkItem(at: url, to: named)
        }
      } catch {
        result(FlutterError(code: "failed", message: error.localizedDescription, details: nil))
        return
      }
      current?.answer(false)
      let save = SaveToFiles()
      save.result = result
      save.copy = named == url ? nil : named
      current = save
      let picker = UIDocumentPickerViewController(forExporting: [named], asCopy: true)
      picker.delegate = save
      top.present(picker, animated: true)
    }

    private func answer(_ saved: Bool) {
      if let copy = copy { try? FileManager.default.removeItem(at: copy) }
      copy = nil
      result?(saved)
      result = nil
      if Self.current === self { Self.current = nil }
    }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
      answer(!urls.isEmpty)
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
      answer(false)
    }
  }
#endif
