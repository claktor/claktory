import Cocoa
import FlutterMacOS

@main
class AppDelegate: FlutterAppDelegate {
  /// The name the person sees ("Claktory", "Claktory Dev"): the bundle's
  /// localized name (en.lproj/InfoPlist.strings), not its file name, which
  /// stays Factory.app so installs and updates find it.
  static var displayName: String {
    let bundle = Bundle.main
    for key in ["CFBundleName", "CFBundleDisplayName"] {
      if let name = bundle.object(forInfoDictionaryKey: key) as? String, !name.isEmpty {
        return name
      }
    }
    return ProcessInfo.processInfo.processName
  }

  override func applicationWillFinishLaunching(_ notification: Notification) {
    // MainMenu.xib says "About APP_NAME", "Hide APP_NAME", "Quit APP_NAME".
    if let menu = NSApp.mainMenu { Self.name(menu) }
    super.applicationWillFinishLaunching(notification)
  }

  private static func name(_ menu: NSMenu) {
    menu.title = menu.title.replacingOccurrences(of: "APP_NAME", with: displayName)
    for item in menu.items {
      item.title = item.title.replacingOccurrences(of: "APP_NAME", with: displayName)
      if let submenu = item.submenu { name(submenu) }
    }
  }

  override func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
    return true
  }

  override func applicationSupportsSecureRestorableState(_ app: NSApplication) -> Bool {
    return true
  }
}
