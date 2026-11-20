import AuthenticationServices
import Cocoa
import CoreImage
import FlutterMacOS

class MainFlutterWindow: NSWindow {
  /// Key for the window's frame in this app's user defaults on this device.
  private static let frameName = "Factory main window"

  override func awakeFromNib() {
    // Before anything reads this app's defaults (the frame, just below).
    SandboxMigration.runForThisApp()
    let flutterViewController = FlutterViewController()
    self.contentViewController = flutterViewController

    // Sidebar-style window (docs/IDENTITY.md): content runs under a transparent
    // title bar, and the traffic lights sit in the sidebar's top row, centered
    // in the 44pt band the views' header bars fill. No toolbar: a toolbar
    // would take every click in the band, and the headers there are
    // clickable. The app drags and zooms the window itself (see `windowChannel`).
    // Hidden in the window, but named in the Window and Dock menus and
    // Mission Control.
    title = AppDelegate.displayName
    styleMask.insert(.fullSizeContentView)
    titlebarAppearsTransparent = true
    titleVisibility = .hidden
    titlebarSeparatorStyle = .none
    minSize = NSSize(width: 800, height: 500)

    // The window reopens where and how big it was. This is kept on the device
    // rather than in the backend: each device has its own screens (AGENTS.md,
    // "Product rules"). The only other thing kept here is unsent drafts (see
    // `deviceChannel`).
    if !setFrameUsingName(Self.frameName) {
      setFrame(Self.defaultFrame(on: screen ?? NSScreen.main), display: true)
    }
    setFrameAutosaveName(Self.frameName)

    RegisterGeneratedPlugins(registry: flutterViewController)
    // The video viewer's player (VideoPlayers.swift).
    VideoPlayers.register(with: flutterViewController.registrar(forPlugin: "VideoPlayers"))

    windowChannel = FlutterMethodChannel(
      name: "factory/window", binaryMessenger: flutterViewController.engine.binaryMessenger)
    windowChannel?.setMethodCallHandler { [weak self] call, result in
      switch call.method {
      case "drag": self?.drag()
      case "zoom": self?.doubleClicked()
      case "pickFolder": return self?.pickFolder(from: call.arguments as? String, result) ?? result(nil)
      case "pickFiles": return self?.pickFiles(result) ?? result([])
      case "takeFiles": self?.takesFiles = call.arguments as? Bool ?? false
      case "band":
        // The app's zoom scales its top band (app/lib/zoom.dart).
        if let height = call.arguments as? Double, height > 0 {
          self?.bandHeight = CGFloat(height)
          self?.placeTrafficLights()
        }
      default: return result(FlutterMethodNotImplemented)
      }
      result(nil)
    }

    // The View menu's zoom items, done by the app (app/lib/zoom.dart).
    menuChannel = FlutterMethodChannel(
      name: "factory/menu", binaryMessenger: flutterViewController.engine.binaryMessenger)

    deviceChannel = FlutterMethodChannel(
      name: "factory/device", binaryMessenger: flutterViewController.engine.binaryMessenger)
    deviceChannel?.setMethodCallHandler { call, result in
      switch call.method {
      case "drafts":
        result(UserDefaults.standard.dictionary(forKey: Self.draftsKey) as? [String: String] ?? [:])
      case "setDrafts":
        UserDefaults.standard.set(call.arguments as? [String: String] ?? [:], forKey: Self.draftsKey)
        result(nil)
      case "connections":
        result(Self.connections.read())
      case "setConnections":
        Self.connections.write(call.arguments as? [String: Any] ?? [:])
        result(nil)
      case "qrCode":
        result(Self.qrCode(call.arguments as? String ?? ""))
      case "webAuth":
        WebAuth.shared.start(call.arguments, result)
      case "cancelWebAuth":
        WebAuth.shared.cancel()
        result(nil)
      default: result(FlutterMethodNotImplemented)
      }
    }

    // AppKit puts the traffic lights back wherever it lays out the title bar.
    for name in [
      NSWindow.didResizeNotification, NSWindow.didEndLiveResizeNotification,
      NSWindow.didExitFullScreenNotification, NSWindow.didBecomeKeyNotification,
      NSWindow.didResignKeyNotification, NSWindow.didChangeScreenNotification,
    ] {
      NotificationCenter.default.addObserver(
        self, selector: #selector(placeTrafficLights), name: name, object: self)
    }
    placeTrafficLights()

    registerForDraggedTypes([.fileURL])

    pasteMonitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .keyUp]) {
      [weak self] event in
      guard let self, Self.isPaste(event) else { return event }
      if event.type == .keyUp {
        // Its key-down was handled here, so Flutter never sees a stray "v".
        return self.pasting ? nil : event
      }
      self.pasting = self.paste(into: event.window ?? self) || self.pasteFiles()
      return self.pasting ? nil : event
    }

    super.awakeFromNib()
  }

  deinit {
    if let pasteMonitor { NSEvent.removeMonitor(pasteMonitor) }
  }

  /// Height of the band at the top of the window: the sidebar's top row and
  /// the views' header bars (`titleBarHeight` in app/lib/identity.dart),
  /// scaled by the app's zoom.
  private var bandHeight: CGFloat = 44
  /// Where the close button sits from the window's left edge, and the gap
  /// from one traffic light to the next.
  private static let trafficLightsLeft: CGFloat = 16
  private static let trafficLightsSpacing: CGFloat = 20

  /// Centers the traffic lights in the top band. Full screen shows them in
  /// its own title bar, so they're left alone there.
  @objc private func placeTrafficLights() {
    guard !styleMask.contains(.fullScreen),
      let close = standardWindowButton(.closeButton),
      let container = close.superview?.superview
    else { return }
    var frame = container.frame
    frame.size.height = bandHeight
    frame.origin.y = self.frame.height - bandHeight
    container.frame = frame
    let buttons: [NSWindow.ButtonType] = [.closeButton, .miniaturizeButton, .zoomButton]
    for (index, type) in buttons.enumerated() {
      guard let button = standardWindowButton(type) else { continue }
      button.setFrameOrigin(
        NSPoint(
          x: Self.trafficLightsLeft + CGFloat(index) * Self.trafficLightsSpacing,
          y: (bandHeight - button.frame.height) / 2))
    }
  }

  private var menuChannel: FlutterMethodChannel?

  /// Actual Size, Zoom In and Zoom Out at the top of the View menu, as in
  /// Safari (added by AppDelegate once the menu is loaded). The app's own keys (app/lib/zoom.dart) get ⌘+, ⌘− and ⌘0
  /// first; these items are how the menu shows them, and take the keys when
  /// nothing in the window has focus.
  static func addZoomItems() {
    guard
      let view = NSApp.mainMenu?.items.first(where: { $0.submenu?.title == "View" })?.submenu,
      view.item(withTitle: "Zoom In") == nil
    else { return }
    let items: [(String, Selector, String, Bool)] = [
      ("Actual Size", #selector(actualSize(_:)), "0", false),
      ("Zoom In", #selector(zoomIn(_:)), "+", false),
      // ⌘= zooms in too, without Shift, as in browsers.
      ("Zoom In", #selector(zoomIn(_:)), "=", true),
      ("Zoom Out", #selector(zoomOut(_:)), "-", false),
    ]
    for (index, (title, action, key, hidden)) in items.enumerated() {
      let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
      item.keyEquivalentModifierMask = .command
      if hidden {
        item.isHidden = true
        item.allowsKeyEquivalentWhenHidden = true
      }
      view.insertItem(item, at: index)
    }
    view.insertItem(.separator(), at: items.count)
  }

  @objc func actualSize(_ sender: Any?) { menuChannel?.invokeMethod("actualSize", arguments: nil) }
  @objc func zoomIn(_ sender: Any?) { menuChannel?.invokeMethod("zoomIn", arguments: nil) }
  @objc func zoomOut(_ sender: Any?) { menuChannel?.invokeMethod("zoomOut", arguments: nil) }

  // Unsent drafts in the agents' message boxes, by agent id (app/lib/drafts.dart),
  // in this app's user defaults on this device: the person's decision, the one
  // exception besides the window's frame to everything living in the backend.
  private var deviceChannel: FlutterMethodChannel?
  private static let draftsKey = "Drafts"

  // Which Factory the app shows (a pairing code, or empty for this Mac's) and
  // the others it has connected to (app/lib/connections.dart): the person's
  // decision, the third thing kept on the device, since a device can't ask
  // a backend where its backend is. Their codes hold tokens, so they're in
  // the Keychain (ConnectionStore.swift).
  private static let connections = ConnectionStore.forThisApp(
    displayName: AppDelegate.displayName)

  /// `text` as a QR code from Core Image (medium error correction): its
  /// modules row by row, top first, 1 for dark; nil if it can't be made.
  private static func qrCode(_ text: String) -> [String: Any]? {
    guard let filter = CIFilter(name: "CIQRCodeGenerator") else { return nil }
    filter.setValue(Data(text.utf8), forKey: "inputMessage")
    filter.setValue("M", forKey: "inputCorrectionLevel")
    guard let image = filter.outputImage else { return nil }
    // One pixel per module, with whatever margin Core Image adds: the code
    // is the box around its dark modules (its finder patterns mark three
    // corners).
    let width = Int(image.extent.width)
    let height = Int(image.extent.height)
    guard width > 0, height > 0 else { return nil }
    var pixels = [UInt8](repeating: 255, count: width * height * 4)
    guard let sRGB = CGColorSpace(name: CGColorSpace.sRGB) else { return nil }
    let context = CIContext(options: [.useSoftwareRenderer: true])
    context.render(
      image, toBitmap: &pixels, rowBytes: width * 4, bounds: image.extent,
      format: .RGBA8, colorSpace: sRGB)
    // Rows come top first; dark is a low red channel.
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

  // The app's header bars and empty bands move the window: Flutter sees the
  // press, and asks for the drag once the pointer moves ("drag") or for the
  // title bar's double-click action ("zoom").
  private var windowChannel: FlutterMethodChannel?

  /// Moves the window with the pointer until the button is released. The
  /// drag swallows the mouse-up, so Flutter is sent one once the button is
  /// up; otherwise it would take the next click for a drag. A spare mouse-up
  /// is harmless to Flutter.
  private func drag() {
    guard let event = currentEvent,
      [.leftMouseDown, .leftMouseDragged].contains(event.type)
    else { return }
    performDrag(with: event)
    releaseFlutterPointer()
  }

  private func releaseFlutterPointer() {
    if NSEvent.pressedMouseButtons & 1 != 0 {
      DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { [weak self] in
        self?.releaseFlutterPointer()
      }
      return
    }
    guard
      let controller = contentViewController as? FlutterViewController,
      let up = NSEvent.mouseEvent(
        with: .leftMouseUp, location: mouseLocationOutsideOfEventStream,
        modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
        windowNumber: windowNumber, context: nil, eventNumber: 0, clickCount: 1,
        pressure: 0)
    else { return }
    controller.mouseUp(with: up)
  }

  /// Asks for a folder in a sheet on this window, starting in `from` when
  /// it exists; answers its path, or nil when cancelled. A new folder can be
  /// made in the sheet.
  private func pickFolder(from: String?, _ result: @escaping FlutterResult) {
    let panel = NSOpenPanel()
    panel.canChooseDirectories = true
    panel.canChooseFiles = false
    panel.canCreateDirectories = true
    panel.allowsMultipleSelection = false
    panel.prompt = "Choose"
    if let from, !from.isEmpty {
      panel.directoryURL = URL(fileURLWithPath: from, isDirectory: true)
    }
    panel.beginSheetModal(for: self) { response in
      result(response == .OK ? panel.url?.path : nil)
    }
  }

  /// What double-clicking a title bar does, as set in System Settings →
  /// Desktop & Dock: zoom (the default), minimize, or nothing.
  private func doubleClicked() {
    switch UserDefaults.standard.string(forKey: "AppleActionOnDoubleClick") {
    case "Minimize": performMiniaturize(nil)
    case "None": break
    default: performZoom(nil)
    }
  }

  // Cmd+V is pasted here rather than by Flutter. Apps that paste for you
  // (dictation like Hex or Wispr Flow, clipboard managers, text expanders)
  // send a synthetic Cmd+V, and Flutter's keyboard handling drops it, so
  // nothing appears (flutter/flutter#184571). Inserting the clipboard's text
  // through the text input client, the way macOS dictation does, works for
  // them and for a real Cmd+V alike.
  private var pasteMonitor: Any?
  /// A Cmd+V key-down was pasted here; its key-up is swallowed too.
  private var pasting = false

  private static func isPaste(_ event: NSEvent) -> Bool {
    let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
    let command = flags.contains(.command) || (event.cgEvent?.flags.contains(.maskCommand) ?? false)
    let others = flags.intersection([.control, .option, .shift])
    let v = event.charactersIgnoringModifiers?.lowercased() == "v" || event.keyCode == 9
    return command && others.isEmpty && v
  }

  /// Inserts the clipboard's text where the text cursor is; false when no
  /// text field has focus or there's no text to paste.
  private func paste(into window: NSWindow) -> Bool {
    guard let text = NSPasteboard.general.string(forType: .string), !text.isEmpty else {
      return false
    }
    let responder = window.firstResponder
    guard
      let client = (responder as? NSTextInputClient)
        ?? (responder as? NSView)?.inputContext?.client
    else { return false }
    client.insertText(text, replacementRange: NSRange(location: NSNotFound, length: 0))
    return true
  }

  // Files dragged onto the window from the Finder go to the message box of
  // the agent showing (app/lib/attachments.dart), while there is one.
  private var takesFiles = false

  private func files(in sender: NSDraggingInfo) -> [String] {
    let urls = sender.draggingPasteboard.readObjects(
      forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]
    return (urls ?? []).map(\.path)
  }

  @objc func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
    guard takesFiles, !files(in: sender).isEmpty else { return [] }
    windowChannel?.invokeMethod("filesOver", arguments: true)
    return .copy
  }

  @objc func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation {
    takesFiles && !files(in: sender).isEmpty ? .copy : []
  }

  @objc func draggingExited(_ sender: NSDraggingInfo?) {
    windowChannel?.invokeMethod("filesOver", arguments: false)
  }

  @objc func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
    let paths = files(in: sender)
    guard takesFiles, !paths.isEmpty else {
      windowChannel?.invokeMethod("filesOver", arguments: false)
      return false
    }
    windowChannel?.invokeMethod("filesDropped", arguments: paths)
    return true
  }

  /// Cmd+V with files (copied in the Finder) or a picture on the clipboard
  /// and no text attaches them, as dropping them would; false when there's
  /// nothing to attach or no message box takes files.
  private func pasteFiles() -> Bool {
    guard takesFiles else { return false }
    let board = NSPasteboard.general
    let urls = board.readObjects(
      forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
    var paths = urls.map(\.path)
    if paths.isEmpty, let image = NSImage(pasteboard: board),
      let tiff = image.tiffRepresentation,
      let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:])
    {
      let folder = FileManager.default.temporaryDirectory
        .appendingPathComponent("Attachments").appendingPathComponent(UUID().uuidString)
      let file = folder.appendingPathComponent("Pasted image.png")
      do {
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try png.write(to: file)
        paths = [file.path]
      } catch { return false }
    }
    guard !paths.isEmpty else { return false }
    windowChannel?.invokeMethod("filesDropped", arguments: paths)
    return true
  }

  /// Asks for files to send in a sheet on this window; answers their
  /// paths, or none when cancelled.
  private func pickFiles(_ result: @escaping FlutterResult) {
    let panel = NSOpenPanel()
    panel.canChooseDirectories = false
    panel.canChooseFiles = true
    panel.allowsMultipleSelection = true
    panel.prompt = "Attach"
    panel.beginSheetModal(for: self) { response in
      result(response == .OK ? panel.urls.map(\.path) : [])
    }
  }

  /// First launch: roomy enough for the sidebar and a readable conversation,
  /// without covering the whole screen. Centered.
  private static func defaultFrame(on screen: NSScreen?) -> NSRect {
    guard let visible = screen?.visibleFrame else {
      return NSRect(x: 0, y: 0, width: 1280, height: 820)
    }
    let width = min(1440, max(800, (visible.width * 0.78).rounded()))
    let height = min(940, max(500, (visible.height * 0.85).rounded()))
    return NSRect(
      x: visible.midX - width / 2, y: visible.midY - height / 2, width: width, height: height)
  }
}

/// The browser step of connecting a service (app/lib/web_auth.dart) when
/// the service sends back to this app's own address (`scheme`) rather than
/// to one the app listens on: the system's sign-in window, which shares the
/// default browser's sign-ins. It answers the address the service sent back
/// to, nil when the person closed it, or an error.
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

  /// Closes the window; the waiting call answers nil.
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
    NSApp.keyWindow ?? NSApp.mainWindow ?? NSApp.windows.first ?? ASPresentationAnchor()
  }
}
