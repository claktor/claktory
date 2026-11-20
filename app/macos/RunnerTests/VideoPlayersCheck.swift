// Checks the video viewer's player (Runner/VideoPlayers.swift) without the
// app: it opens a video, reads its state, seeks, takes a frame and plays.
//
//   cd app && flutter build macos --debug && cd macos && \
//     swiftc -o /tmp/video-players-check \
//       -F ../build/macos/Build/Products/Debug \
//       -Xlinker -rpath -Xlinker ../build/macos/Build/Products/Debug \
//       Runner/VideoPlayers.swift RunnerTests/VideoPlayersCheck.swift \
//     && /tmp/video-players-check
//
// It makes its own short video with AVAssetWriter in a temporary folder.
import AVFoundation
import Foundation

@main
enum VideoPlayersCheck {
  static func main() throws {
    var failures = 0
    func check(_ ok: Bool, _ what: String, line: Int = #line) {
      if !ok {
        failures += 1
        print("FAILED (line \(line)): \(what)")
      }
    }
    func wait(_ seconds: Double, until done: () -> Bool = { false }) {
      let end = Date().addingTimeInterval(seconds)
      while Date() < end, !done() {
        RunLoop.main.run(until: Date().addingTimeInterval(0.02))
      }
    }

    let file = try makeVideo(seconds: 3, width: 320, height: 180)
    defer { try? FileManager.default.removeItem(at: file.deletingLastPathComponent()) }

    // A file that isn't a video says why.
    let bad = file.deletingLastPathComponent().appendingPathComponent("bad.mp4")
    try Data("not a video".utf8).write(to: bad)
    var badError: String?
    let badPlayer = VideoPlayer(url: bad)
    var badDone = false
    badPlayer.load { badError = $0; badDone = true }
    wait(10) { badDone }
    check(badError != nil, "a file that isn't a video fails: \(String(describing: badError))")
    badPlayer.close()

    let player = VideoPlayer(url: file)
    var states: [[String: Any]] = []
    player.changed = { states.append($0) }
    var frames = 0
    player.frameAvailable = { frames += 1 }
    var loaded = false
    var error: String?
    player.load { error = $0; loaded = true }
    wait(10) { loaded }
    check(loaded && error == nil, "loads: \(String(describing: error))")
    let state = player.state()
    check(abs((state["duration"] as? Double ?? 0) - 3) < 0.1, "duration \(state)")
    check(state["width"] as? Double == 320 && state["height"] as? Double == 180, "size \(state)")
    check(state["playing"] as? Bool == false, "paused at first")
    check(state["quarterTurns"] as? Int == 0, "not turned")

    // The first frame comes without playing.
    wait(3) { frames > 0 }
    check(frames > 0, "a first frame")
    check(player.copyPixelBuffer() != nil, "the frame can be drawn")

    var sought = false
    player.seek(to: 1.5) { sought = true }
    wait(5) { sought }
    check(abs((player.state()["position"] as? Double ?? 0) - 1.5) < 0.05, "seeks: \(player.state())")

    player.play()
    wait(1)
    let playing = player.state()
    check(playing["playing"] as? Bool == true, "plays")
    check((playing["position"] as? Double ?? 0) > 1.8, "moves on: \(playing)")
    check(states.contains { $0["playing"] as? Bool == true }, "says it's playing")
    let framesPlaying = frames
    wait(0.5)
    check(frames > framesPlaying + 5, "frames while playing: \(frames - framesPlaying)")

    // Plays to the end and stops there; play starts it again.
    wait(3) { player.state()["playing"] as? Bool == false }
    check(player.state()["playing"] as? Bool == false, "stops at the end")
    player.play()
    wait(0.5)
    check((player.state()["position"] as? Double ?? 9) < 1, "plays again from the start")
    player.pause()
    wait(0.3)
    check(player.state()["playing"] as? Bool == false, "pauses")
    player.close()

    print(failures == 0 ? "VideoPlayers: all checks passed" : "VideoPlayers: \(failures) failed")
    exit(failures == 0 ? 0 : 1)
  }

  /// A video of [seconds] at 30 frames a second, grey getting lighter.
  static func makeVideo(seconds: Int, width: Int, height: Int) throws -> URL {
    let folder = FileManager.default.temporaryDirectory
      .appendingPathComponent("video-players-check-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    let url = folder.appendingPathComponent("clip.mp4")
    let writer = try AVAssetWriter(outputURL: url, fileType: .mp4)
    let input = AVAssetWriterInput(mediaType: .video, outputSettings: [
      AVVideoCodecKey: AVVideoCodecType.h264,
      AVVideoWidthKey: width,
      AVVideoHeightKey: height,
    ])
    let adaptor = AVAssetWriterInputPixelBufferAdaptor(
      assetWriterInput: input,
      sourcePixelBufferAttributes: [
        kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
        kCVPixelBufferWidthKey as String: width,
        kCVPixelBufferHeightKey as String: height,
      ])
    writer.add(input)
    writer.startWriting()
    writer.startSession(atSourceTime: .zero)
    for frame in 0..<(seconds * 30) {
      while !input.isReadyForMoreMediaData { usleep(1000) }
      var buffer: CVPixelBuffer?
      CVPixelBufferPoolCreatePixelBuffer(nil, adaptor.pixelBufferPool!, &buffer)
      CVPixelBufferLockBaseAddress(buffer!, [])
      let bytes = CVPixelBufferGetBaseAddress(buffer!)!
      memset(bytes, Int32(frame * 255 / (seconds * 30)), CVPixelBufferGetDataSize(buffer!))
      CVPixelBufferUnlockBaseAddress(buffer!, [])
      adaptor.append(buffer!, withPresentationTime: CMTime(value: CMTimeValue(frame), timescale: 30))
    }
    input.markAsFinished()
    var done = false
    writer.finishWriting { done = true }
    while !done { usleep(1000) }
    if writer.status != .completed { throw writer.error ?? NSError(domain: "check", code: 1) }
    return url
  }
}
