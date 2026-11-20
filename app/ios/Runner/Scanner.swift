import AVFoundation
import Flutter
import UIKit
import Vision

/// Reads QR codes, from the camera's frames or from an image, with Vision:
/// the same reading either way, so a test can check it on an image the
/// Simulator, which has no camera, can make.
enum CodeReader {
  /// The texts of the QR codes in `handler`'s image.
  static func read(_ handler: VNImageRequestHandler) -> [String] {
    let request = VNDetectBarcodesRequest()
    request.symbologies = [.qr]
    #if targetEnvironment(simulator)
      // The newer readers need the Neural Engine, which the Simulator
      // doesn't have; the first reads QR codes just as well.
      request.revision = VNDetectBarcodesRequestRevision1
    #endif
    guard (try? handler.perform([request])) != nil else { return [] }
    return (request.results ?? []).compactMap(\.payloadStringValue)
  }

  static func read(_ image: CGImage) -> [String] {
    read(VNImageRequestHandler(cgImage: image, options: [:]))
  }

  static func read(_ buffer: CVPixelBuffer) -> [String] {
    read(VNImageRequestHandler(cvPixelBuffer: buffer, options: [:]))
  }
}

/// The camera, for the pairing screen (app/lib/scanner.dart): whether the
/// app may use it, asking for it, and the app's page in Settings.
enum Camera {
  /// "granted", "undetermined", "denied", "restricted", or "unavailable"
  /// where there's no camera (the Simulator).
  static func access() -> String {
    guard AVCaptureDevice.default(for: .video) != nil else { return "unavailable" }
    switch AVCaptureDevice.authorizationStatus(for: .video) {
    case .authorized: return "granted"
    case .notDetermined: return "undetermined"
    case .restricted: return "restricted"
    default: return "denied"
    }
  }

  /// Asks the person (the system's prompt, with NSCameraUsageDescription)
  /// when they haven't answered yet; answers whether it's allowed.
  static func request(_ done: @escaping (Bool) -> Void) {
    AVCaptureDevice.requestAccess(for: .video) { granted in
      DispatchQueue.main.async { done(granted) }
    }
  }

  static func openSettings() {
    if let url = URL(string: UIApplication.openSettingsURLString) {
      UIApplication.shared.open(url)
    }
  }
}

/// Makes the camera views the pairing screen embeds ("claktory/scanner").
class ScannerFactory: NSObject, FlutterPlatformViewFactory {
  private let messenger: FlutterBinaryMessenger

  init(messenger: FlutterBinaryMessenger) {
    self.messenger = messenger
  }

  func create(withFrame frame: CGRect, viewIdentifier viewId: Int64, arguments args: Any?)
    -> FlutterPlatformView
  {
    let channel = FlutterMethodChannel(
      name: "claktory/scanner/\(viewId)", binaryMessenger: messenger)
    return ScannerView(frame: frame, channel: channel)
  }
}

/// The camera's picture, filling the view, telling Flutter each QR code it
/// reads ("scanned", its text) once until another is read.
class ScannerView: NSObject, FlutterPlatformView, AVCaptureVideoDataOutputSampleBufferDelegate {
  private let preview = PreviewView()
  private let channel: FlutterMethodChannel
  private let session = AVCaptureSession()
  private let queue = DispatchQueue(label: "claktory.scanner")
  private var last = ""
  private var lastRead = Date.distantPast

  init(frame: CGRect, channel: FlutterMethodChannel) {
    self.channel = channel
    super.init()
    preview.frame = frame
    preview.backgroundColor = .black
    preview.layer.masksToBounds = true
    preview.previewLayer.session = session
    preview.previewLayer.videoGravity = .resizeAspectFill
    queue.async { [weak self] in self?.start() }
  }

  deinit {
    let session = session
    queue.async { session.stopRunning() }
  }

  func view() -> UIView { preview }

  private func start() {
    guard let camera = AVCaptureDevice.default(for: .video),
      let input = try? AVCaptureDeviceInput(device: camera),
      session.canAddInput(input)
    else { return }
    session.beginConfiguration()
    session.sessionPreset = .high
    session.addInput(input)
    let output = AVCaptureVideoDataOutput()
    output.alwaysDiscardsLateVideoFrames = true
    output.setSampleBufferDelegate(self, queue: queue)
    if session.canAddOutput(output) { session.addOutput(output) }
    session.commitConfiguration()
    session.startRunning()
  }

  func captureOutput(
    _ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer,
    from connection: AVCaptureConnection
  ) {
    // A few reads a second are plenty, and spare the battery.
    let now = Date()
    guard now.timeIntervalSince(lastRead) > 0.2,
      let buffer = CMSampleBufferGetImageBuffer(sampleBuffer)
    else { return }
    lastRead = now
    guard let code = CodeReader.read(buffer).first, code != last else { return }
    last = code
    DispatchQueue.main.async { [channel] in
      channel.invokeMethod("scanned", arguments: code)
    }
  }
}

/// A view whose layer is the camera's preview, so it follows the view's
/// size.
class PreviewView: UIView {
  override class var layerClass: AnyClass { AVCaptureVideoPreviewLayer.self }
  var previewLayer: AVCaptureVideoPreviewLayer { layer as! AVCaptureVideoPreviewLayer }
}
