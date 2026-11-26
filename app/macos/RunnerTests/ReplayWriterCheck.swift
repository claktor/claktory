// Checks History's video encoder (Runner/ReplayWriter.swift's
// ReplayEncoder) without the app or Flutter:
//
//   cd app/macos && swiftc -o /tmp/replay-writer-check \
//     Runner/ReplayWriter.swift RunnerTests/ReplayWriterCheck.swift \
//     && /tmp/replay-writer-check
//
// REPLAY_KEEP=<path> keeps a copy of the MP4 it made.
//
// It makes a short MP4 in a fresh share folder, reads it back with
// AVFoundation (size, frames, length, a frame's colour), checks the file
// holds no metadata boxes, that bad sizes and frames are refused, and that
// cancel deletes the folder.
import AVFoundation
import Foundation

@main
enum ReplayWriterCheck {
  static func main() throws {
    var failures = 0
    func check(_ ok: Bool, _ what: String, line: Int = #line) {
      if !ok {
        failures += 1
        print("FAILED (line \(line)): \(what)")
      }
    }

    // Refused: odd, tiny or huge sizes, and a wrong fps.
    func refused(_ width: Int, _ height: Int, _ fps: Int) -> Bool {
      let url = try! ReplayEncoder.newFile()
      defer { try? FileManager.default.removeItem(at: url.deletingLastPathComponent()) }
      return (try? ReplayEncoder(url: url, width: width, height: height, fps: fps)) == nil
    }
    check(refused(101, 100, 30), "an odd width")
    check(refused(8, 8, 30), "too small")
    check(refused(1922, 1080, 30), "too big")
    check(refused(100, 100, 0), "no fps")
    check(refused(100, 100, 120), "too many fps")

    // In the share folder, a fresh one each time.
    let url = try ReplayEncoder.newFile()
    let folder = url.deletingLastPathComponent()
    check(folder.deletingLastPathComponent().lastPathComponent == "claktory-share", "share folder")
    check(url.lastPathComponent == "claktory-replay.mp4", "its name")
    check(try ReplayEncoder.newFile().deletingLastPathComponent() != folder, "a fresh folder")

    let width = 64
    let height = 48
    let encoder = try ReplayEncoder(url: url, width: width, height: height, fps: 30)
    // A wrong frame size is refused and leaves it going.
    do {
      try encoder.append(rgba: Data(count: 10))
      check(false, "a short frame")
    } catch {}
    // 45 frames of pure red (RGBA).
    var red = Data(count: width * height * 4)
    for i in stride(from: 0, to: red.count, by: 4) {
      red[i] = 255
      red[i + 3] = 255
    }
    for _ in 0..<45 { try encoder.append(rgba: red) }
    check(encoder.frames == 45, "45 frames in")
    var finished: Error?? = .none
    encoder.finish { finished = .some($0) }
    let end = Date().addingTimeInterval(20)
    while finished == nil, Date() < end {
      RunLoop.main.run(until: Date().addingTimeInterval(0.02))
    }
    check(finished != nil, "finished")
    if case .some(.some(let error)) = finished { check(false, "finish failed: \(error)") }

    let asset = AVURLAsset(url: url)
    let track = asset.tracks(withMediaType: .video).first
    check(track != nil, "a video track")
    if let track = track {
      check(track.naturalSize == CGSize(width: width, height: height), "its size \(track.naturalSize)")
      let seconds = CMTimeGetSeconds(asset.duration)
      check(abs(seconds - 1.5) < 0.05, "1.5 s long, not \(seconds)")
    }
    check(asset.metadata.isEmpty, "no metadata: \(asset.metadata)")
    check(asset.commonMetadata.isEmpty, "no common metadata")

    // A frame reads back red (H.264 is lossy: near enough).
    let images = AVAssetImageGenerator(asset: asset)
    images.requestedTimeToleranceBefore = .zero
    images.requestedTimeToleranceAfter = .zero
    if let image = try? images.copyCGImage(at: CMTime(value: 10, timescale: 30), actualTime: nil) {
      var pixel = [UInt8](repeating: 0, count: 4)
      let context = CGContext(
        data: &pixel, width: 1, height: 1, bitsPerComponent: 8, bytesPerRow: 4,
        space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
      context.draw(image, in: CGRect(x: -32, y: -24, width: image.width, height: image.height))
      check(pixel[0] > 200 && pixel[1] < 60 && pixel[2] < 60, "red, not \(pixel)")
    } else {
      check(false, "a frame read back")
    }

    // No metadata boxes anywhere in the file: no udta, meta or ©-tags.
    let bytes = try Data(contentsOf: url)
    for name in ["udta", "meta", "\u{A9}too", "\u{A9}xyz", "\u{A9}mak", "\u{A9}mod"] {
      let tag = Data(name.unicodeScalars.map { UInt8(truncatingIfNeeded: $0.value) })
      check(bytes.range(of: tag) == nil, "no \(name) box")
    }

    // Cancel deletes the folder.
    let other = try ReplayEncoder.newFile()
    let cancelled = try ReplayEncoder(url: other, width: 32, height: 32, fps: 30)
    try cancelled.append(rgba: Data(count: 32 * 32 * 4))
    cancelled.cancel()
    check(
      !FileManager.default.fileExists(atPath: other.deletingLastPathComponent().path),
      "cancel deletes its folder")

    // REPLAY_KEEP=path keeps a copy (app/test/fixtures/replay.mp4 is one).
    if let keep = ProcessInfo.processInfo.environment["REPLAY_KEEP"] {
      try? FileManager.default.removeItem(atPath: keep)
      try FileManager.default.copyItem(at: url, to: URL(fileURLWithPath: keep))
    }
    try? FileManager.default.removeItem(at: folder)
    if failures == 0 {
      print("ReplayWriterCheck: all passed")
    } else {
      print("ReplayWriterCheck: \(failures) failed")
      exit(1)
    }
  }
}
