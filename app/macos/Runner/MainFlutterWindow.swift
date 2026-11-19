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

    windowChannel = FlutterMethodChannel(
      name: "factory/window", binaryMessenger: flutterViewController.engine.binaryMessenger)
    windowChannel?.setMethodCallHandler { [weak self] call, result in
      switch call.method {
      case "drag": self?.drag()
      case "zoom": self?.doubleClicked()
      case "pickFolder": return self?.pickFolder(from: call.arguments as? String, result) ?? result(nil)
      default: return result(FlutterMethodNotImplemented)
      }
      result(nil)
    }

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
        result(UserDefaults.standard.dictionary(forKey: Self.connectionsKey) ?? [:])
      case "setConnections":
        UserDefaults.standard.set(call.arguments as? [String: Any] ?? [:], forKey: Self.connectionsKey)
        result(nil)
      case "qrCode":
        result(Self.qrCode(call.arguments as? String ?? ""))
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

    pasteMonitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .keyUp]) {
      [weak self] event in
      guard let self, Self.isPaste(event) else { return event }
      if event.type == .keyUp {
        // Its key-down was handled here, so Flutter never sees a stray "v".
        return self.pasting ? nil : event
      }
      self.pasting = self.paste(into: event.window ?? self)
      return self.pasting ? nil : event
    }

    super.awakeFromNib()
  }

  deinit {
    if let pasteMonitor { NSEvent.removeMonitor(pasteMonitor) }
  }

  /// Height of the band at the top of the window: the sidebar's top row and
  /// the views' header bars (`titleBarHeight` in app/lib/identity.dart).
  private static let bandHeight: CGFloat = 44
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
    frame.size.height = Self.bandHeight
    frame.origin.y = self.frame.height - Self.bandHeight
    container.frame = frame
    let buttons: [NSWindow.ButtonType] = [.closeButton, .miniaturizeButton, .zoomButton]
    for (index, type) in buttons.enumerated() {
      guard let button = standardWindowButton(type) else { continue }
      button.setFrameOrigin(
        NSPoint(
          x: Self.trafficLightsLeft + CGFloat(index) * Self.trafficLightsSpacing,
          y: (Self.bandHeight - button.frame.height) / 2))
    }
  }

  // Unsent drafts in the agents' message boxes, by agent id (app/lib/drafts.dart),
  // in this app's user defaults on this device: the person's decision, the one
  // exception besides the window's frame to everything living in the backend.
  private var deviceChannel: FlutterMethodChannel?
  private static let draftsKey = "Drafts"

  // Which Factory the app shows (a pairing code, or empty for this Mac's) and
  // the others it has connected to (app/lib/connections.dart): the person's
  // decision, the third thing kept on the device, since a device can't ask
  // a backend where its backend is.
  private static let connectionsKey = "Connections"

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
