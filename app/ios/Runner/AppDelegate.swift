import AuthenticationServices
import CoreImage
import Flutter
import PhotosUI
import Security
import UIKit
import UniformTypeIdentifiers

@main
@objc class AppDelegate: FlutterAppDelegate, FlutterImplicitEngineDelegate {
  override func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
  ) -> Bool {
    return super.application(application, didFinishLaunchingWithOptions: launchOptions)
  }

  func didInitializeImplicitFlutterEngine(_ engineBridge: FlutterImplicitEngineBridge) {
    GeneratedPluginRegistrant.register(with: engineBridge.pluginRegistry)
    let messenger = engineBridge.pluginRegistry.registrar(forPlugin: "Device")!.messenger()
    // The camera's picture that scans pairing codes (Scanner.swift).
    let scanner = engineBridge.pluginRegistry.registrar(forPlugin: "Scanner")!
    scanner.register(ScannerFactory(messenger: scanner.messenger()), withId: "claktory/scanner")
    // The video viewer's player (app/macos/Runner/VideoPlayers.swift).
    VideoPlayers.register(with: engineBridge.pluginRegistry.registrar(forPlugin: "VideoPlayers")!)
    deviceChannel = FlutterMethodChannel(name: "factory/device", binaryMessenger: messenger)
    deviceChannel?.setMethodCallHandler { call, result in
      switch call.method {
      case "drafts":
        result(UserDefaults.standard.dictionary(forKey: Self.draftsKey) as? [String: String] ?? [:])
      case "setDrafts":
        UserDefaults.standard.set(call.arguments as? [String: String] ?? [:], forKey: Self.draftsKey)
        result(nil)
      case "connections":
        result(Keychain.read() ?? [:])
      case "setConnections":
        Keychain.write(call.arguments as? [String: Any] ?? [:])
        result(nil)
      case "qrCode":
        result(Self.qrCode(call.arguments as? String ?? ""))
      case "cameraAccess":
        result(Camera.access())
      case "requestCamera":
        Camera.request { result($0) }
      case "openCameraSettings":
        Camera.openSettings()
        result(nil)
      case "pickAttachments":
        Attach.shared.pick(result)
      case "webAuth":
        WebAuth.shared.start(call.arguments, result)
      case "cancelWebAuth":
        WebAuth.shared.cancel()
        result(nil)
      default: result(FlutterMethodNotImplemented)
      }
    }
  }

  // What the app keeps on the device (app/lib/drafts.dart, connections.dart),
  // as on the Mac (MainFlutterWindow.swift): unsent drafts by agent id, and
  // which Factory it shows with this device's key for each.
  private var deviceChannel: FlutterMethodChannel?
  private static let draftsKey = "Drafts"

  /// `text` as a QR code from Core Image: its modules row by row, 1 for dark.
  private static func qrCode(_ text: String) -> [String: Any]? {
    guard let filter = CIFilter(name: "CIQRCodeGenerator") else { return nil }
    filter.setValue(Data(text.utf8), forKey: "inputMessage")
    filter.setValue("M", forKey: "inputCorrectionLevel")
    guard let image = filter.outputImage else { return nil }
    let width = Int(image.extent.width)
    let height = Int(image.extent.height)
    guard width > 0, height > 0, let sRGB = CGColorSpace(name: CGColorSpace.sRGB) else { return nil }
    var pixels = [UInt8](repeating: 255, count: width * height * 4)
    CIContext(options: [.useSoftwareRenderer: true]).render(
      image, toBitmap: &pixels, rowBytes: width * 4, bounds: image.extent,
      format: .RGBA8, colorSpace: sRGB)
    func dark(_ x: Int, _ y: Int) -> Bool { pixels[(y * width + x) * 4] < 128 }
    var left = width, top = height, right = -1, bottom = -1
    for y in 0..<height {
      for x in 0..<width where dark(x, y) {
        left = min(left, x)
        right = max(right, x)
        top = min(top, y)
        bottom = max(bottom, y)
      }
    }
    let size = right - left + 1
    guard size > 0, bottom - top + 1 == size else { return nil }
    var modules = [UInt8](repeating: 0, count: size * size)
    for y in 0..<size {
      for x in 0..<size {
        modules[y * size + x] = dark(left + x, top + y) ? 1 : 0
      }
    }
    return ["size": size, "modules": FlutterStandardTypedData(bytes: Data(modules))]
  }
}

/// The connections (pairing codes and this device's iroh key for each) in
/// the Keychain, only on this device and only once it's been unlocked: the
/// keys are what a backend knows this phone by.
enum Keychain {
  private static let query: [String: Any] = [
    kSecClass as String: kSecClassGenericPassword,
    kSecAttrService as String: "Claktory connections",
    kSecAttrAccount as String: "connections",
  ]

  static func read() -> [String: Any]? {
    var lookup = query
    lookup[kSecReturnData as String] = true
    lookup[kSecMatchLimit as String] = kSecMatchLimitOne
    var item: CFTypeRef?
    guard SecItemCopyMatching(lookup as CFDictionary, &item) == errSecSuccess,
      let data = item as? Data
    else { return nil }
    return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
  }

  static func write(_ connections: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: connections) else { return }
    let update: [String: Any] = [
      kSecValueData as String: data,
      kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
    ]
    if SecItemUpdate(query as CFDictionary, update as CFDictionary) == errSecItemNotFound {
      SecItemAdd(query.merging(update) { $1 } as CFDictionary, nil)
    }
  }
}

/// Where the person attaches files from (app/lib/attachments.dart): the
/// system's menu (Choose from Photos, Choose from Files, and Paste image when the
/// clipboard holds a picture), then its picker. What's chosen is copied
/// into this app's temporary folder, and the copies' paths answered.
final class Attach: NSObject, PHPickerViewControllerDelegate, UIDocumentPickerDelegate {
  static let shared = Attach()

  private var result: FlutterResult?

  func pick(_ result: @escaping FlutterResult) {
    guard self.result == nil, let presenter = Self.presenter() else { return result([]) }
    self.result = result
    let menu = UIAlertController(title: nil, message: nil, preferredStyle: .actionSheet)
    menu.addAction(
      UIAlertAction(title: "Choose from Photos", style: .default) { _ in self.photos(presenter) })
    menu.addAction(
      UIAlertAction(title: "Choose from Files", style: .default) { _ in self.files(presenter) })
    if UIPasteboard.general.hasImages {
      menu.addAction(UIAlertAction(title: "Paste image", style: .default) { _ in self.paste() })
    }
    menu.addAction(UIAlertAction(title: "Cancel", style: .cancel) { _ in self.finish([]) })
    presenter.present(menu, animated: true)
  }

  private func finish(_ paths: [String]) {
    result?(paths)
    result = nil
  }

  private func photos(_ presenter: UIViewController) {
    var configuration = PHPickerConfiguration()
    configuration.selectionLimit = 0
    let picker = PHPickerViewController(configuration: configuration)
    picker.delegate = self
    presenter.present(picker, animated: true)
  }

  func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
    picker.dismiss(animated: true)
    let group = DispatchGroup()
    let lock = NSLock()
    var paths = [Int: String]()
    for (index, item) in results.enumerated() {
      let provider = item.itemProvider
      // JPEG when the photo offers it: agents read it more readily than HEIC.
      let types = provider.registeredTypeIdentifiers
      guard
        let type = types.first(where: { $0 == UTType.jpeg.identifier })
          ?? types.first(where: { $0 == UTType.png.identifier }) ?? types.first
      else { continue }
      let ext = UTType(type)?.preferredFilenameExtension
      let name = (provider.suggestedName ?? "Photo") + (ext.map { ".\($0)" } ?? "")
      group.enter()
      provider.loadFileRepresentation(forTypeIdentifier: type) { url, _ in
        // The file is gone once this returns, so it's copied now.
        if let url, let copy = Self.copy(url, as: name) {
          lock.lock()
          paths[index] = copy
          lock.unlock()
        }
        group.leave()
      }
    }
    group.notify(queue: .main) { self.finish(paths.keys.sorted().compactMap { paths[$0] }) }
  }

  private func files(_ presenter: UIViewController) {
    let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.item], asCopy: true)
    picker.allowsMultipleSelection = true
    picker.delegate = self
    presenter.present(picker, animated: true)
  }

  func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
    // Copies already, made for this app.
    finish(urls.map(\.path))
  }

  func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
    finish([])
  }

  private func paste() {
    guard let png = UIPasteboard.general.image?.pngData(), let folder = Self.folder() else {
      return finish([])
    }
    let file = folder.appendingPathComponent("Pasted image.png")
    finish((try? png.write(to: file)) == nil ? [] : [file.path])
  }

  /// A new folder of its own in the temporary folder, so names never clash.
  private static func folder() -> URL? {
    let folder = FileManager.default.temporaryDirectory
      .appendingPathComponent("Attachments").appendingPathComponent(UUID().uuidString)
    return (try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true))
      == nil ? nil : folder
  }

  private static func copy(_ url: URL, as name: String) -> String? {
    guard let folder = folder() else { return nil }
    let file = folder.appendingPathComponent(name)
    return (try? FileManager.default.copyItem(at: url, to: file)) == nil ? nil : file.path
  }

  /// What's on screen, to present the menu and pickers over.
  private static func presenter() -> UIViewController? {
    let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
    let scene = scenes.first { $0.activationState == .foregroundActive } ?? scenes.first
    var top = scene?.windows.first { $0.isKeyWindow }?.rootViewController
      ?? scene?.windows.first?.rootViewController
    while let presented = top?.presentedViewController { top = presented }
    return top
  }
}

/// The browser step of connecting a service (app/lib/web_auth.dart): the
/// service's page in the system's sign-in sheet, which shares Safari's
/// cookies so a service already signed in to needs no password again. It
/// answers the address the service sent back to (`scheme`), nil when the
/// person closed the sheet, or an error.
final class WebAuth: NSObject, ASWebAuthenticationPresentationContextProviding {
  static let shared = WebAuth()

  private var session: ASWebAuthenticationSession?
  private var result: FlutterResult?

  func start(_ arguments: Any?, _ result: @escaping FlutterResult) {
    guard let arguments = arguments as? [String: Any],
      let url = (arguments["url"] as? String).flatMap(URL.init(string:)),
      let scheme = arguments["scheme"] as? String
    else {
      return result(FlutterError(code: "arguments", message: "No address to open.", details: nil))
    }
    cancel()
    let session = ASWebAuthenticationSession(url: url, callbackURLScheme: scheme) {
      [weak self] callback, error in
      DispatchQueue.main.async { self?.finished(callback, error) }
    }
    session.presentationContextProvider = self
    self.session = session
    self.result = result
    if !session.start() {
      finished(nil, NSError(domain: "WebAuth", code: 0, userInfo: [
        NSLocalizedDescriptionKey: "Couldn't open the sign-in page.",
      ]))
    }
  }

  /// Closes the sheet; the waiting call answers nil.
  func cancel() {
    session?.cancel()
    finished(nil, nil)
  }

  private func finished(_ callback: URL?, _ error: Error?) {
    guard let result else { return }
    self.result = nil
    session = nil
    if let callback {
      result(callback.absoluteString)
    } else if let error = error as? ASWebAuthenticationSessionError, error.code == .canceledLogin {
      result(nil)
    } else if let error {
      result(FlutterError(code: "failed", message: error.localizedDescription, details: nil))
    } else {
      result(nil)
    }
  }

  func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
    let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
    let scene = scenes.first { $0.activationState == .foregroundActive } ?? scenes.first
    return scene?.windows.first { $0.isKeyWindow } ?? scene?.windows.first ?? ASPresentationAnchor()
  }
}
