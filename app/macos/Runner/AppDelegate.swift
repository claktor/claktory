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
    MainFlutterWindow.addZoomItems()
    super.applicationWillFinishLaunching(notification)
  }

  private static func name(_ menu: NSMenu) {
    menu.title = menu.title.replacingOccurrences(of: "APP_NAME", with: displayName)
    for item in menu.items {
      item.title = item.title.replacingOccurrences(of: "APP_NAME", with: displayName)
      if let submenu = item.submenu { name(submenu) }
    }
  }

  /// A link for this app (claktory://add?code=…), clicked or opened with
  /// `open`: passed on to the window.
  override func application(_ application: NSApplication, open urls: [URL]) {
    Links.open(urls)
  }

  override func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
    return true
  }

  /// Quitting stops this Mac's Claktory first when the person chose that
  /// (friend bug #27): the window's Dart says when it's done.
  override func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
    return Engine.shouldTerminate(sender)
  }

  override func applicationSupportsSecureRestorableState(_ app: NSApplication) -> Bool {
    return true
  }
}

/// Links the app is opened with (app/lib/links.dart). Those that come
/// before the window's Flutter asks for them (the link that launched the
/// app) wait; after that each is passed on as it comes.
enum Links {
  private static var channel: FlutterMethodChannel?
  private static var waiting: [String] = []
  private static var listening = false

  static func register(with messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: "factory/links", binaryMessenger: messenger)
    channel.setMethodCallHandler { call, result in
      switch call.method {
      case "listen":
        listening = true
        result(waiting)
        waiting = []
      default:
        result(FlutterMethodNotImplemented)
      }
    }
    Self.channel = channel
  }

  static func open(_ urls: [URL]) {
    for url in urls {
      if listening, let channel {
        channel.invokeMethod("opened", arguments: url.absoluteString)
      } else {
        waiting.append(url.absoluteString)
      }
    }
  }
}
