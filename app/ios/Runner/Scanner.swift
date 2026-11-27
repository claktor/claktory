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
    if let pretend = Pretend.current() { return pretend }
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
    if let pretend = Pretend.current() {
      Pretend.answer(pretend, done)
      return
    }
    AVCaptureDevice.requestAccess(for: .video) { granted in
      DispatchQueue.main.async { done(granted) }
    }
  }

  static func openSettings() {
    if let url = URL(string: UIApplication.openSettingsURLString) {
      UIApplication.shared.open(url)
    }
  }

  /// QA's way to reach the camera's screens in the Simulator, which has no
  /// camera (area 5a, as LocalNetwork.swift's): any build but the App
  /// Store's reads the file Library/ClaktoryCamera in the app's container,
  /// "granted", "denied", "restricted" or "undetermined". While it says
  /// undetermined, a request waits, as the system's prompt would, until the
  /// file says granted or denied.
  ///
  ///   echo undetermined > "$(xcrun simctl get_app_container booted com.claktory.app.dev data)/Library/ClaktoryCamera"
  enum Pretend {
    static func current() -> String? {
      guard Bundle.main.bundleIdentifier != AppStoreBuild.bundleId,
        let library = FileManager.default.urls(for: .libraryDirectory, in: .userDomainMask).first,
        let said = try? String(contentsOf: library.appendingPathComponent("ClaktoryCamera"))
      else { return nil }
      let value = said.trimmingCharacters(in: .whitespacesAndNewlines)
      return ["granted", "denied", "restricted", "undetermined"].contains(value) ? value : nil
    }

    static func answer(_ pretend: String, _ done: @escaping (Bool) -> Void) {
      if pretend != "undetermined" {
        DispatchQueue.main.async { done(pretend == "granted") }
        return
      }
      DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
        switch current() {
        case "undetermined"?: answer("undetermined", done)
        case let value?: done(value == "granted")
        case nil: done(false)
        }
      }
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
    // An iPad's camera stops while the app shares the screen (Split View,
    // Slide Over, Stage Manager beside another app), or while another app
    // has it: Flutter says so over the picture, which stays black, and
    // takes it back when the camera does.
    let center = NotificationCenter.default
    center.addObserver(
      self, selector: #selector(interrupted(_:)),
      name: AVCaptureSession.wasInterruptedNotification, object: session)
    center.addObserver(
      self, selector: #selector(resumed(_:)),
      name: AVCaptureSession.interruptionEndedNotification, object: session)
    queue.async { [weak self] in self?.start() }
  }

  deinit {
    NotificationCenter.default.removeObserver(self)
    let session = session
    queue.async { session.stopRunning() }
  }

  /// What `unavailable` says of [reason]: "multitasking" while the app shares
  /// the screen, "inUse" while another app has the camera, nil for an
  /// interruption the person needn't act on (a call, audio).
  static func unavailableReason(_ reason: AVCaptureSession.InterruptionReason?) -> String? {
    switch reason {
    case .videoDeviceNotAvailableWithMultipleForegroundApps: return "multitasking"
    case .videoDeviceInUseByAnotherClient: return "inUse"
    default: return nil
    }
  }

  @objc private func interrupted(_ note: Notification) {
    let raw = note.userInfo?[AVCaptureSessionInterruptionReasonKey] as? Int
    guard
      let reason = Self.unavailableReason(
        raw.flatMap(AVCaptureSession.InterruptionReason.init(rawValue:)))
    else { return }
    DispatchQueue.main.async { [channel] in
      channel.invokeMethod("unavailable", arguments: ["reason": reason])
    }
  }

  @objc private func resumed(_ note: Notification) {
    DispatchQueue.main.async { [channel] in
      channel.invokeMethod("available", arguments: nil)
    }
  }

  func view() -> UIView { preview }

  private func start() {
    guard let camera = AVCaptureDevice.default(for: .video),
      let input = try? AVCaptureDeviceInput(device: camera),
      session.canAddInput(input)
    else { return }
    session.beginConfiguration()
    // An iPad that can keeps the camera in Split View, Slide Over and Stage
    // Manager, so the code reads there too and "not available in this
    // window" is only for one that can't. A QR code needs nothing heavy.
    // From iOS 18 without the multitasking entitlement, which an app built
    // for iOS 15 would need before it (Apple's "Accessing the camera while
    // multitasking on iPad").
    if #available(iOS 18.0, *), session.isMultitaskingCameraAccessSupported {
      session.isMultitaskingCameraAccessEnabled = true
    }
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
