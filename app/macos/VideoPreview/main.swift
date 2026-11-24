// Claktory's video preview maker (#379, Polpo's file-previews.md).
//
// factoryd runs it to make a phone-sized copy of a video: H.264 at about
// 800 kbit/s, at most 30 frames a second, fitting a screen either way round,
// AAC audio, with its index at the front (fast start) so it plays while it
// loads. AVAssetWriter with an explicit bitrate: AVFoundation's export
// presets make these videos bigger, not smaller.
//
//   claktory-video-preview IN OUT SHORT LONG
//
// IN is the original, OUT where the preview goes (written whole, or not at
// all), SHORT and LONG the screen's sides in pixels (it never goes past
// 720 x 1280). On success it prints
// one line of JSON, {"width": …, "height": …, "seconds": …}, the preview's
// size as it shows and its length, and exits 0; otherwise it says why on
// stderr and exits 1. It reads only IN, writes only OUT, and runs nothing.
//
//   claktory-video-preview pdf IN OUT PAGE SHORT LONG
//
// Renders page PAGE (1 is the first) of the PDF IN as a PNG at OUT, sized
// to fill SHORT x LONG either way round (LONG at most 4096, 16 million
// pixels at most), white behind it, with CoreGraphics' drawPDFPage: what
// the page draws, never scripts, forms' actions or links. A PDF locked with
// a password is refused, never unlocked. On success it prints
// {"width": …, "height": …, "pages": …, "page_width": …, "page_height": …}
// (the page's size in points, turned as it shows) and exits 0; a page past
// the end prints {"pages": …} and exits 3; otherwise it says why on stderr
// and exits 1.
//
// Either way it shuts itself in before it opens IN (sandbox_init, deny by
// default; Fugo's #379 review): from then on it reads IN and the system's
// own files only, writes only in OUT's folder (factoryd makes a fresh one
// for each job), and reaches no network, other file or other program, so
// a hostile video or PDF parsed here can't reach Claktory's data. One it
// can't shut in, it refuses to run.
//
//   claktory-video-preview probe IN OUT SECRET PORT
//
// Shuts itself in as for IN and OUT, then tries what a hostile file would:
// reading SECRET, listing the home folder, writing beside OUT's folder and
// connecting to 127.0.0.1:PORT, and prints what got through, as JSON (the
// test of the sandbox: everything but IN and OUT must be refused).

import AVFoundation
import CoreGraphics
import Foundation
import ImageIO

/// The preview's video bitrate, bits a second.
let videoBitrate = 800_000
/// Its audio's.
let audioBitrate = 64_000
/// The most frames a second it keeps.
let mostFramesPerSecond: Int32 = 30

func fail(_ why: String) -> Never {
    FileHandle.standardError.write(Data((why + "\n").utf8))
    exit(1)
}

/// `size` scaled down to fit `short` x `long` either way round, never up,
/// in even pixels (H.264 wants them).
func fit(_ size: CGSize, short: CGFloat, long: CGFloat) -> CGSize {
    let (across, down) = size.width <= size.height ? (short, long) : (long, short)
    let scale = min(across / size.width, down / size.height, 1)
    let even = { (length: CGFloat) in max(2, (length * scale / 2).rounded() * 2) }
    return CGSize(width: even(size.width), height: even(size.height))
}

@_silgen_name("sandbox_init")
private func sandbox_init(
    _ profile: UnsafePointer<CChar>, _ flags: UInt64, _ errorbuf: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
) -> Int32

/// `path` with every link followed, as the sandbox sees it.
func real(_ path: String) -> String {
    guard let resolved = realpath(path, nil) else { return path }
    defer { free(resolved) }
    return String(cString: resolved)
}

/// From here on: reads only `input` and the system's own files, reads and
/// writes only in `output`'s folder, and no network, no other program.
/// `input`'s real path, the one to open: following a link is a read of the
/// link's own path, which the sandbox refuses.
func shutIn(input: String, output: String) -> String {
    let input = real(input)
    let folder = real((output as NSString).deletingLastPathComponent)
    var directory: ObjCBool = false
    guard FileManager.default.fileExists(atPath: folder, isDirectory: &directory), directory.boolValue, folder != "/" else {
        fail("OUT's folder isn't one of its own")
    }
    let quoted = { (path: String) in
        "\"" + path.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"") + "\""
    }
    let profile = """
        (version 1)
        (deny default)
        (import "system.sb")
        (allow file-read* (literal \(quoted(input))))
        (allow file-read* file-write* (subpath \(quoted(folder))))
        (allow file-read* (literal \(quoted(real(Bundle.main.executablePath ?? CommandLine.arguments[0])))))
        (allow user-preference-read)
        (allow iokit-open (iokit-user-client-class "IOSurfaceRootUserClient" "IOSurfaceAcceleratorClient" "AGXDeviceUserClient" "AppleAVE2UserClient"))
        """
    var error: UnsafeMutablePointer<CChar>?
    guard sandbox_init(profile, 0, &error) == 0 else {
        fail("can't shut itself in: \(error.map { String(cString: $0) } ?? "unknown")")
    }
    return input
}

/// `probe IN OUT SECRET PORT`: what gets out of the sandbox.
func probe(_ arguments: [String]) -> Never {
    guard arguments.count == 6, let port = UInt16(arguments[5]) else { fail("usage: claktory-video-preview probe IN OUT SECRET PORT") }
    let (output, secret) = (arguments[3], arguments[4])
    let input = shutIn(input: arguments[2], output: output)
    let beside = ((real((output as NSString).deletingLastPathComponent) as NSString).deletingLastPathComponent as NSString)
        .appendingPathComponent("escaped-\(getpid())")
    let wrote = FileManager.default.createFile(atPath: beside, contents: Data("out".utf8))
    let connected: Bool = {
        let socket = Darwin.socket(AF_INET, SOCK_STREAM, 0)
        guard socket >= 0 else { return false }
        defer { close(socket) }
        var address = sockaddr_in()
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = port.bigEndian
        address.sin_addr.s_addr = inet_addr("127.0.0.1")
        return withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(socket, $0, socklen_t(MemoryLayout<sockaddr_in>.size)) == 0 }
        }
    }()
    let results: [String: Bool] = [
        "read_input": FileManager.default.contents(atPath: input) != nil,
        "write_output": FileManager.default.createFile(atPath: output, contents: Data("in".utf8)),
        "read_secret": FileManager.default.contents(atPath: secret) != nil,
        "list_home": (try? FileManager.default.contentsOfDirectory(atPath: NSHomeDirectory())) != nil,
        "write_beside": wrote,
        "connect": connected,
    ]
    let json = results.keys.sorted().map { "\"\($0)\": \(results[$0]!)" }.joined(separator: ", ")
    print("{\(json)}")
    exit(0)
}

/// The most pixels a page is rendered at, either way and in all.
let pageSideMost: CGFloat = 4096
let pagePixelsMost: CGFloat = 16_000_000

/// `pdf IN OUT PAGE SHORT LONG`: one page as a PNG.
func renderPage(_ arguments: [String]) -> Never {
    guard arguments.count == 7, let number = Int(arguments[4]), (1...100_000).contains(number),
        let short = Double(arguments[5]), let long = Double(arguments[6]), short > 0, long >= short, long <= Double(pageSideMost)
    else {
        fail("usage: claktory-video-preview pdf IN OUT PAGE SHORT LONG (LONG at most 4096)")
    }
    let output = URL(fileURLWithPath: arguments[3])
    try? FileManager.default.removeItem(at: output)
    let input = shutIn(input: arguments[2], output: arguments[3])
    guard let document = CGPDFDocument(URL(fileURLWithPath: input) as CFURL) else { fail("it isn't a PDF this Mac can read") }
    guard !document.isEncrypted else { fail("it's locked with a password") }
    let pages = document.numberOfPages
    guard number <= pages else {
        print("{\"pages\": \(pages)}")
        exit(3)
    }
    guard let page = document.page(at: number) else { fail("it has no page \(number)") }
    let box = page.getBoxRect(.cropBox)
    let turned = (page.rotationAngle / 90) % 2 != 0
    let shown = turned ? CGSize(width: box.height, height: box.width) : box.size
    guard shown.width >= 1, shown.height >= 1 else { fail("its page has no size") }
    // Filling the screen the way round the page is, up or down.
    let (across, down) = shown.width <= shown.height ? (CGFloat(short), CGFloat(long)) : (CGFloat(long), CGFloat(short))
    var scale = min(across / shown.width, down / shown.height)
    scale = min(scale, (pagePixelsMost / (shown.width * shown.height)).squareRoot())
    let width = max(1, Int((shown.width * scale).rounded()))
    let height = max(1, Int((shown.height * scale).rounded()))
    guard let space = CGColorSpace(name: CGColorSpace.sRGB),
        let context = CGContext(
            data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0, space: space,
            bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)
    else { fail("can't make a picture \(width) x \(height)") }
    context.setFillColor(CGColor(gray: 1, alpha: 1))
    context.fill(CGRect(x: 0, y: 0, width: width, height: height))
    context.interpolationQuality = .high
    context.scaleBy(x: CGFloat(width) / shown.width, y: CGFloat(height) / shown.height)
    context.concatenate(page.getDrawingTransform(.cropBox, rect: CGRect(origin: .zero, size: shown), rotate: 0, preserveAspectRatio: true))
    context.drawPDFPage(page)
    guard let image = context.makeImage(),
        let destination = CGImageDestinationCreateWithURL(output as CFURL, "public.png" as CFString, 1, nil)
    else { fail("can't write it") }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination) else {
        try? FileManager.default.removeItem(at: output)
        fail("couldn't write it")
    }
    let points = { (length: CGFloat) in Int(length.rounded()) }
    print("{\"width\": \(width), \"height\": \(height), \"pages\": \(pages), \"page_width\": \(points(shown.width)), \"page_height\": \(points(shown.height))}")
    exit(0)
}

if CommandLine.arguments.count > 1 && CommandLine.arguments[1] == "pdf" {
    renderPage(CommandLine.arguments)
}
if CommandLine.arguments.count > 1 && CommandLine.arguments[1] == "probe" {
    probe(CommandLine.arguments)
}

let arguments = CommandLine.arguments
guard arguments.count == 5, let short = Double(arguments[3]), let long = Double(arguments[4]), short > 0, long >= short else {
    fail("usage: claktory-video-preview IN OUT SHORT LONG")
}
let output = URL(fileURLWithPath: arguments[2])
try? FileManager.default.removeItem(at: output)
let input = URL(fileURLWithPath: shutIn(input: arguments[1], output: arguments[2]))

let asset = AVURLAsset(url: input)

/// What it needs to know of the original, loaded from it.
struct Original {
    let duration: CMTime
    let video: AVAssetTrack
    let audio: AVAssetTrack?
    let natural: CGSize
    let transform: CGAffineTransform
    let rate: Float
}

func load() async throws -> Original {
    let duration = try await asset.load(.duration)
    guard let video = try await asset.loadTracks(withMediaType: .video).first else { fail("it has no video track") }
    let audio = try await asset.loadTracks(withMediaType: .audio).first
    let (natural, transform, rate) = try await video.load(.naturalSize, .preferredTransform, .nominalFrameRate)
    return Original(duration: duration, video: video, audio: audio, natural: natural, transform: transform, rate: rate)
}

/// `load`, waited for here: the rest runs on this thread.
func loaded() -> Original {
    final class Box: @unchecked Sendable { var result: Result<Original, Error>? }
    let box = Box()
    let done = DispatchSemaphore(value: 0)
    Task {
        do { box.result = .success(try await load()) } catch { box.result = .failure(error) }
        done.signal()
    }
    done.wait()
    switch box.result {
    case .success(let original): return original
    case .failure(let error): fail("can't read it: \(error.localizedDescription)")
    case nil: fail("can't read it")
    }
}

let original = loaded()
let (duration, video, audio, natural, transform, rate) =
    (original.duration, original.video, original.audio, original.natural, original.transform, original.rate)
// As it shows: turned as its transform says.
let shown = natural.applying(transform)
let display = CGSize(width: abs(shown.width), height: abs(shown.height))
guard display.width >= 1, display.height >= 1 else { fail("its picture has no size") }
// At most 720p, whatever the screen: at this bitrate more pixels only blur.
let target = fit(display, short: CGFloat(min(short, 720)), long: CGFloat(min(long, 1280)))
let scale = target.width / display.width
let framesPerSecond = rate >= 1 ? min(Int32(rate.rounded()), mostFramesPerSecond) : mostFramesPerSecond

// The picture, turned upright, scaled and at most 30 frames a second.
let composition = AVMutableVideoComposition()
composition.renderSize = target
composition.frameDuration = CMTime(value: 1, timescale: framesPerSecond)
let instruction = AVMutableVideoCompositionInstruction()
instruction.timeRange = CMTimeRange(start: .zero, duration: duration)
let layer = AVMutableVideoCompositionLayerInstruction(assetTrack: video)
// Turned about its corner, as a file's matrix may be with no move after it
// (ffmpeg's rotation tag): moved back to where the picture is drawn, or
// every frame lands outside it and comes out black.
let turned = CGRect(origin: .zero, size: natural).applying(transform)
let upright = transform.concatenating(CGAffineTransform(translationX: -turned.minX, y: -turned.minY))
layer.setTransform(upright.concatenating(CGAffineTransform(scaleX: scale, y: scale)), at: .zero)
instruction.layerInstructions = [layer]
composition.instructions = [instruction]

let reader: AVAssetReader
let writer: AVAssetWriter
do {
    reader = try AVAssetReader(asset: asset)
    writer = try AVAssetWriter(outputURL: output, fileType: .mp4)
} catch {
    fail("can't start: \(error.localizedDescription)")
}
writer.shouldOptimizeForNetworkUse = true

let pictures = AVAssetReaderVideoCompositionOutput(
    videoTracks: [video],
    videoSettings: [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange])
pictures.videoComposition = composition
pictures.alwaysCopiesSampleData = false
guard reader.canAdd(pictures) else { fail("can't read its pictures") }
reader.add(pictures)
let videoInput = AVAssetWriterInput(mediaType: .video, outputSettings: [
    AVVideoCodecKey: AVVideoCodecType.h264,
    AVVideoWidthKey: Int(target.width),
    AVVideoHeightKey: Int(target.height),
    AVVideoCompressionPropertiesKey: [
        AVVideoAverageBitRateKey: videoBitrate,
        AVVideoExpectedSourceFrameRateKey: Int(framesPerSecond),
        AVVideoMaxKeyFrameIntervalKey: Int(framesPerSecond) * 2,
        AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
    ],
])
videoInput.expectsMediaDataInRealTime = false
guard writer.canAdd(videoInput) else { fail("can't write its pictures") }
writer.add(videoInput)

var sound: (AVAssetReaderTrackOutput, AVAssetWriterInput)?
if let audio {
    let samples = AVAssetReaderTrackOutput(track: audio, outputSettings: [AVFormatIDKey: kAudioFormatLinearPCM])
    let audioInput = AVAssetWriterInput(mediaType: .audio, outputSettings: [
        AVFormatIDKey: kAudioFormatMPEG4AAC,
        AVNumberOfChannelsKey: 2,
        AVSampleRateKey: 44_100,
        AVEncoderBitRateKey: audioBitrate,
    ])
    audioInput.expectsMediaDataInRealTime = false
    if reader.canAdd(samples), writer.canAdd(audioInput) {
        reader.add(samples)
        writer.add(audioInput)
        sound = (samples, audioInput)
    }
}

guard reader.startReading() else { fail("can't read it: \(reader.error?.localizedDescription ?? "unknown")") }
guard writer.startWriting() else { fail("can't write it: \(writer.error?.localizedDescription ?? "unknown")") }
writer.startSession(atSourceTime: .zero)

/// Copies `from` into `into` as the writer takes it, then marks it done.
func pump(_ from: AVAssetReaderOutput, into: AVAssetWriterInput, reader: AVAssetReader, label: String, group: DispatchGroup) {
    group.enter()
    let queue = DispatchQueue(label: label)
    // Used only on `queue` from here on.
    nonisolated(unsafe) let (from, into, reader) = (from, into, reader)
    into.requestMediaDataWhenReady(on: queue) {
        while into.isReadyForMoreMediaData {
            guard let sample = from.copyNextSampleBuffer() else {
                into.markAsFinished()
                group.leave()
                return
            }
            if !into.append(sample) {
                reader.cancelReading()
                into.markAsFinished()
                group.leave()
                return
            }
        }
    }
}

let group = DispatchGroup()
pump(pictures, into: videoInput, reader: reader, label: "video", group: group)
if let (samples, audioInput) = sound {
    pump(samples, into: audioInput, reader: reader, label: "audio", group: group)
}
group.wait()
if reader.status == .failed || writer.status == .failed {
    writer.cancelWriting()
    try? FileManager.default.removeItem(at: output)
    fail("couldn't make it: \((writer.error ?? reader.error)?.localizedDescription ?? "unknown")")
}
let finished = DispatchSemaphore(value: 0)
writer.finishWriting { finished.signal() }
finished.wait()
guard writer.status == .completed else {
    try? FileManager.default.removeItem(at: output)
    fail("couldn't finish it: \(writer.error?.localizedDescription ?? "unknown")")
}
print("{\"width\": \(Int(target.width)), \"height\": \(Int(target.height)), \"seconds\": \(CMTimeGetSeconds(duration))}")
