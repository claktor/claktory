import CoreImage
import Flutter
import Security
import UIKit

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
