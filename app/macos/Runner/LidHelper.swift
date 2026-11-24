// Keeping agents working with a Mac laptop's lid closed (#371,
// coordination/security/lid-closed.md §I). Claktory's lid helper is a
// launchd daemon in this app's own bundle
// (Contents/Library/LaunchDaemons/com.claktory.lid-helper.plist, put there by
// the release; Claktory Dev and ad hoc builds carry none). The app registers
// it with SMAppService only when the person turns the setting on, here on the
// Mac; macOS then asks for their approval, with an administrator password, in
// System Settings → Login Items, which the app opens. Turning it off
// unregisters it. Nothing else: the helper's state while it runs is
// factoryd's to read, over XPC.
//
// factory/lid_helper:
// - "status" → "unavailable" (macOS before 14), "notFound" (this app's
//   bundle carries no helper: Contents/Library/LaunchDaemons' plist and
//   Contents/Helpers/claktory-lid-helper), "notRegistered" (also never
//   registered), "requiresApproval" or "enabled";
// - "register" and "unregister" → the status after; they fail with code
//   "failed" and macOS's words;
// - "openLoginItems" opens System Settings → General → Login Items.

import FlutterMacOS
import ServiceManagement

enum LidHelper {
  static let plistName = "com.claktory.lid-helper.plist"

  static func register(with messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: "factory/lid_helper", binaryMessenger: messenger)
    channel.setMethodCallHandler { call, result in
      guard #available(macOS 14.0, *) else {
        return result(call.method == "status" ? "unavailable" : unavailable)
      }
      let service = SMAppService.daemon(plistName: plistName)
      switch call.method {
      case "status":
        result(carried ? name(service.status) : "notFound")
      case "register":
        guard carried else {
          return result(FlutterError(
            code: "failed", message: "This Mac app has no helper to install.", details: nil))
        }
        do {
          try service.register()
          result(name(service.status))
        } catch {
          // Waiting for the person's approval isn't a failure: the status
          // says so.
          if service.status == .requiresApproval {
            return result(name(service.status))
          }
          result(FlutterError(code: "failed", message: error.localizedDescription, details: nil))
        }
      case "unregister":
        service.unregister { error in
          DispatchQueue.main.async {
            if let error, service.status != .notRegistered {
              result(FlutterError(code: "failed", message: error.localizedDescription, details: nil))
            } else {
              result(name(service.status))
            }
          }
        }
      case "openLoginItems":
        SMAppService.openSystemSettingsLoginItems()
        result(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }

  /// This app carries the helper: the release puts its plist and program
  /// in the bundle; Claktory Dev and ad hoc builds have neither. Read from
  /// the bundle, since SMAppService says .notFound both for an app without
  /// it and for one whose helper was never registered (Pericolo's
  /// rehearsal).
  private static var carried: Bool {
    let contents = Bundle.main.bundleURL.appendingPathComponent("Contents")
    let files = FileManager.default
    return files.fileExists(
      atPath: contents.appendingPathComponent("Library/LaunchDaemons/\(plistName)").path)
      && files.isExecutableFile(
        atPath: contents.appendingPathComponent("Helpers/claktory-lid-helper").path)
  }

  private static let unavailable = FlutterError(
    code: "failed", message: "This needs macOS 14 or later.", details: nil)

  @available(macOS 14.0, *)
  private static func name(_ status: SMAppService.Status) -> String {
    switch status {
    case .notRegistered: return "notRegistered"
    case .enabled: return "enabled"
    case .requiresApproval: return "requiresApproval"
    // Carried (see [carried]) but never registered: macOS knows of no job
    // yet.
    case .notFound: return "notRegistered"
    @unknown default: return "notRegistered"
    }
  }
}
