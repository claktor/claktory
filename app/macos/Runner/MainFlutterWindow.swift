import AuthenticationServices
import Cocoa
import CoreImage
import CoreText
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
    // The PDF viewer (PaperViewer.swift).
    Paper.register(with: flutterViewController.registrar(forPlugin: "Paper"))
    // Setting Claktory up from the app downloaded as a disk image (Setup.swift).
    Setup.register(with: flutterViewController.engine.binaryMessenger)
    // Links the app opens, such as claktory://add?code=… (AppDelegate.swift).
    Links.register(with: flutterViewController.engine.binaryMessenger)
    // A Claktory to connect to, from an agent setting it up (SetupLink.swift).
    SetupLink.register(with: flutterViewController.engine.binaryMessenger)
    // Voice messages' microphone (Recorder.swift).
    Recorder.register(with: flutterViewController.engine.binaryMessenger)
    // And playing them back (AudioPlayer.swift).
    AudioPlayer.register(with: flutterViewController.engine.binaryMessenger)
    // History's video export, its frames into an MP4 (ReplayWriter.swift).
    ReplayWriter.register(with: flutterViewController.engine.binaryMessenger)
    // The network path, to reconnect the moment it changes (NetworkPath.swift).
    NetworkPath.register(with: flutterViewController.engine.binaryMessenger)
    // Copying a secret, such as a device's add code, hidden from clipboard
    // managers (SecretClipboard.swift).
    SecretClipboard.register(with: flutterViewController.engine.binaryMessenger)
    // The keys of end-to-end direct messages, in the data-protection
    // keychain (DmKeys.swift).
    DmKeys.register(with: flutterViewController.engine.binaryMessenger)
    // This Mac's account key once its own Claktory signed the app in (#529):
    // kept as a phone's, in the data-protection keychain, this device only.
    DmKeys.register(
      with: flutterViewController.engine.binaryMessenger,
      store: DmKeyStore(
        group: DmKeyStore.entitledGroup(), dataProtection: true,
        service: "com.claktory.account-key"),
      channel: "factory/account-key")
    // The system's notifications, and whether the person is using the app
    // (Notifications.swift).
    Notifications.register(with: flutterViewController.engine.binaryMessenger, window: self)
    // Browser control's broker, idle until the app starts it (BrowserBridge.swift).
    BrowserBridge.register(with: flutterViewController.engine.binaryMessenger)

    windowChannel = FlutterMethodChannel(
      name: "factory/window", binaryMessenger: flutterViewController.engine.binaryMessenger)
    windowChannel?.setMethodCallHandler { [weak self] call, result in
      switch call.method {
      case "drag": self?.drag()
      case "zoom": self?.doubleClicked()
      case "pickFolder": return self?.pickFolder(from: call.arguments as? String, result) ?? result(nil)
      case "pickFiles": return self?.pickFiles(result) ?? result([])
      case "takeFiles": self?.takesFiles = call.arguments as? Bool ?? false
      case "devMark": self?.showDevMark()
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

    // Whether anyone can see the window, and the person's Reduce motion
    // (app/lib/sight.dart): nothing in the window moves while it's covered,
    // minimized or hidden, and Flutter's macOS embedder doesn't pass Reduce
    // motion on.
    sightChannel = FlutterMethodChannel(
      name: "factory/sight", binaryMessenger: flutterViewController.engine.binaryMessenger)
    sightChannel?.setMethodCallHandler { [weak self] call, result in
      guard let self else { return result(FlutterMethodNotImplemented) }
      switch call.method {
      case "state":
        self.lastSight = self.sight
        result(self.lastSight)
      // The person's pointer is over the window, so it's seen whatever
      // occlusion said: the next real change is told from there.
      case "seen":
        self.lastSight["shown"] = true
        self.nudgeLifecycle(seen: true)
        result(nil)
      default: result(FlutterMethodNotImplemented)
      }
    }
    lifecycleMessenger = flutterViewController.engine.binaryMessenger
    // Occlusion alone goes stale (flutter#155977): a window uncovered while
    // another app is in front could stay "unseen" until clicked (friend
    // bugs #31 and #40). So it's read again on every change that may
    // uncover it (another app coming to the front among them), and a while
    // after each.
    for name in [
      NSWindow.didChangeOcclusionStateNotification, NSWindow.didBecomeKeyNotification,
      NSWindow.didResignKeyNotification,
      NSWindow.didMiniaturizeNotification, NSWindow.didDeminiaturizeNotification,
    ] {
      NotificationCenter.default.addObserver(
        self, selector: #selector(sightChanged), name: name, object: self)
    }
    for name in [
      NSApplication.didChangeOcclusionStateNotification,
      NSApplication.didBecomeActiveNotification, NSApplication.didResignActiveNotification,
    ] {
      NotificationCenter.default.addObserver(
        self, selector: #selector(sightChanged), name: name, object: NSApp)
    }
    for name in [
      NSWorkspace.accessibilityDisplayOptionsDidChangeNotification,
      NSWorkspace.activeSpaceDidChangeNotification,
      NSWorkspace.didActivateApplicationNotification,
    ] {
      NSWorkspace.shared.notificationCenter.addObserver(
        self, selector: #selector(sightChanged), name: name, object: nil)
    }
    // And the pointer over the window says it's seen, whichever app is in
    // front: Flutter's own hover follows only the key window.
    let pointer = SightPointer { [weak self] in self?.pointerSeen() }
    sightPointer = pointer
    flutterViewController.view.addTrackingArea(
      NSTrackingArea(
        rect: .zero, options: [.mouseEnteredAndExited, .mouseMoved, .activeAlways, .inVisibleRect],
        owner: pointer, userInfo: nil))

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
        do {
          result(try Self.connections.read())
        } catch let failure as StoreFailure {
          result(FlutterError(code: failure.code, message: failure.message, details: nil))
        } catch {
          result(FlutterError(code: "credentials_unavailable", message: "\(error)", details: nil))
        }
      case "setConnections":
        do {
          try Self.connections.write(call.arguments as? [String: Any] ?? [:])
          result(nil)
        } catch let failure as StoreFailure {
          result(FlutterError(code: failure.code, message: failure.message, details: nil))
        } catch {
          result(FlutterError(code: "credentials_write_failed", message: "\(error)", details: nil))
        }
      case "resetConnections":
        // Only when the person chooses Start over.
        do {
          try Self.connections.reset()
          result(nil)
        } catch let failure as StoreFailure {
          result(FlutterError(code: failure.code, message: failure.message, details: nil))
        } catch {
          result(FlutterError(code: "credentials_reset_failed", message: "\(error)", details: nil))
        }
      case "defaultBrowser":
        // The bundle id of the app that opens web pages (lib/pipe_shares.dart:
        // Safari can't open a shared website's *.localhost address).
        let page = URL(string: "http://localhost/")!
        let app = NSWorkspace.shared.urlForApplication(toOpen: page)
        result(app.flatMap { Bundle(url: $0)?.bundleIdentifier } ?? "")
      case "qrCode":
        result(Self.qrCode(call.arguments as? String ?? ""))
      case "webAuth":
        WebAuth.shared.start(call.arguments, result)
      case "cancelWebAuth":
        WebAuth.shared.cancel()
        result(nil)
      case "shareFiles":
        // The share menu for files the app fetched (lib/share_files.dart,
        // #418): AirDrop, Messages, Mail…
        ShareFiles.show(
          call.arguments as? [String: Any] ?? [:], in: flutterViewController.view, result)
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

    // Any build but the real Claktory (#501) wears the DEV VERSION mark,
    // whatever it was built with: a debug or profile build, or a copy
    // given another bundle id.
    if Bundle.main.bundleIdentifier != "dev.factory.app" { showDevMark() }

    super.awakeFromNib()
  }

  deinit {
    if let pasteMonitor { NSEvent.removeMonitor(pasteMonitor) }
    NSWorkspace.shared.notificationCenter.removeObserver(self)
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

  private var sightChannel: FlutterMethodChannel?
  private var lastSight: [String: Bool] = [:]
  private var lifecycleMessenger: FlutterBinaryMessenger?
  /// The tracking area's owner, which it doesn't keep.
  private var sightPointer: SightPointer?
  private var lastPointerSeen = Date.distantPast
  /// Looks again while other windows cover this one (`tellSight`).
  private var coverWatch: Timer?

  private var sight: [String: Bool] {
    [
      "shown": seen,
      "reduceMotion": NSWorkspace.shared.accessibilityDisplayShouldReduceMotion,
    ]
  }

  /// Shown while any of the window is on screen. The key window is shown;
  /// otherwise the windows in front of it are measured, and any of it left
  /// uncovered is shown. Its occlusion state is only the fallback, when the
  /// window server can't be read: it goes stale (flutter#155977: "covered"
  /// while uncovered, friend bugs #31 and #40), and it counts only opaque
  /// windows as covering, so behind a Finder window, whose sidebar lets
  /// what's under it blur through, it stays "visible" and everything that
  /// changes draws for no one (Abbacchio's #384 d).
  private var seen: Bool {
    if isMiniaturized || !isVisible || NSApp.isHidden || !isOnActiveSpace { return false }
    if isKeyWindow { return true }
    return Self.uncovered(windowNumber) ?? occlusionState.contains(.visible)
  }

  /// Whether any of window `number` on screen is left uncovered by other
  /// apps' ordinary windows in front of it, as the window server has them
  /// now (their bounds only: no permission is needed); nil when it can't
  /// say. Only ordinary windows (layer 0) count: the menu bar and the Dock
  /// never hide all of it, and a see-through overlay above everything (a
  /// screen tool's) must not stop the window drawing.
  private static func uncovered(_ number: Int) -> Bool? {
    guard number > 0,
      let own = CGWindowListCopyWindowInfo(.optionIncludingWindow, CGWindowID(number))
        as? [[String: Any]],
      let frame = own.first.flatMap(bounds)
    else { return nil }
    let infront =
      CGWindowListCopyWindowInfo(.optionOnScreenAboveWindow, CGWindowID(number))
      as? [[String: Any]] ?? []
    let me = Int(ProcessInfo.processInfo.processIdentifier)
    let cover = infront.filter {
      (($0[kCGWindowAlpha as String] as? NSNumber)?.doubleValue ?? 1) > 0
        && ($0[kCGWindowLayer as String] as? NSNumber)?.intValue == 0
        && ($0[kCGWindowOwnerPID as String] as? NSNumber)?.intValue != me
    }.compactMap(bounds)
    // The screens, in the window server's coordinates: y down from the top
    // of the main screen.
    let top = NSScreen.screens.first?.frame.maxY ?? 0
    let screens = NSScreen.screens.map {
      CGRect(x: $0.frame.minX, y: top - $0.frame.maxY, width: $0.frame.width, height: $0.frame.height)
    }
    return anyUncovered(frame, by: cover, on: screens)
  }

  private static func bounds(_ window: [String: Any]) -> CGRect? {
    guard let bounds = window[kCGWindowBounds as String] as? NSDictionary else { return nil }
    return CGRect(dictionaryRepresentation: bounds as CFDictionary)
  }

  /// Whether a point of a grid over `frame` is on a screen and outside
  /// every rectangle of `cover`.
  static func anyUncovered(_ frame: CGRect, by cover: [CGRect], on screens: [CGRect]) -> Bool {
    let inner = frame.insetBy(dx: 1, dy: 1)
    guard !inner.isEmpty else { return false }
    let steps = 16
    for column in 0...steps {
      for row in 0...steps {
        let point = CGPoint(
          x: inner.minX + inner.width * CGFloat(column) / CGFloat(steps),
          y: inner.minY + inner.height * CGFloat(row) / CGFloat(steps))
        if screens.contains(where: { $0.contains(point) })
          && !cover.contains(where: { $0.contains(point) })
        {
          return true
        }
      }
    }
    return false
  }

  /// Flutter's engine reads the app's occlusion state too, and goes stale
  /// with it: it then says the app is hidden, and Flutter draws nothing at
  /// all, so nothing new shows until Claktory is brought to the front
  /// (friend bugs #31 and #40); or it says it's seen while covered, and
  /// everything that changes draws for no one. So the app hears what this
  /// window's sight says, as the engine would say it: resumed in front,
  /// inactive behind another app, hidden when not [seen]. Told on every
  /// read, the moments after a change included, so it has the last word
  /// over the engine's own; Flutter ignores a state it's already in.
  private func nudgeLifecycle(seen: Bool) {
    let state =
      !seen
      ? "AppLifecycleState.hidden"
      : NSApp.isActive ? "AppLifecycleState.resumed" : "AppLifecycleState.inactive"
    lifecycleMessenger?.send(onChannel: "flutter/lifecycle", message: state.data(using: .utf8))
  }

  /// The pointer is over the window: it's seen, whatever occlusion said.
  private func pointerSeen() {
    let now = Date()
    // Moves come many times a second; once a second is plenty.
    guard now.timeIntervalSince(lastPointerSeen) >= 1 else { return }
    lastPointerSeen = now
    nudgeLifecycle(seen: true)
    guard lastSight["shown"] != true else { return }
    var next = sight
    next["shown"] = true
    lastSight = next
    sightChannel?.invokeMethod("sight", arguments: next)
  }

  @objc private func sightChanged(_ notification: Notification) {
    tellSight()
    // macOS may settle the occlusion state after the notification: read
    // it again a moment later, and once more after that.
    for delay in [0.5, 2.0] {
      DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
        self?.tellSight()
      }
    }
  }

  /// Tells the app what's seen, when it changed since it was last told.
  /// While other windows cover it, it looks again every second: one
  /// moved away tells this window nothing (its occlusion may never have
  /// changed), and what's under it must show at once (friend bug #40).
  private func tellSight() {
    let now = sight
    nudgeLifecycle(seen: now["shown"] == true)
    let covered =
      now["shown"] != true && isVisible && !isMiniaturized && !NSApp.isHidden && isOnActiveSpace
    if covered, coverWatch == nil {
      let watch = Timer(timeInterval: 1, repeats: true) { [weak self] _ in self?.tellSight() }
      watch.tolerance = 0.25
      RunLoop.main.add(watch, forMode: .common)
      coverWatch = watch
    } else if !covered, let watch = coverWatch {
      watch.invalidate()
      coverWatch = nil
    }
    guard now != lastSight else { return }
    lastSight = now
    sightChannel?.invokeMethod("sight", arguments: now)
  }

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

  /// Claktory Dev's faint "DEV VERSION" across the window (app/lib/main.dart
  /// asks for it in dev builds only). Drawn here rather than by Flutter: a
  /// Flutter-painted mark over a native view, such as the PDF viewer, takes
  /// its clicks and wheel, whatever IgnorePointer says.
  private func showDevMark() {
    guard let flutterView = contentViewController?.view,
      !flutterView.subviews.contains(where: { $0 is DevMarkView })
    else { return }
    let mark = DevMarkView(frame: flutterView.bounds)
    mark.autoresizingMask = [.width, .height]
    flutterView.addSubview(mark)
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

  // Files dragged onto the window from the Finder go to the message box
  // showing (app/lib/attachments.dart), while it takes files.
  private var takesFiles = false

  private func files(in sender: NSDraggingInfo) -> [String] {
    let urls = sender.draggingPasteboard.readObjects(
      forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]
    return (urls ?? []).map(\.path)
  }

  /// Where the drag is, in the app's own coordinates (points from the
  /// window's content's top left), so a card under it takes what's dropped
  /// even when its field doesn't have the keys (#511).
  private func dragPoint(_ sender: NSDraggingInfo) -> [String: Double] {
    guard let view = contentView else { return [:] }
    let point = view.convert(sender.draggingLocation, from: nil)
    let y = view.isFlipped ? point.y : view.bounds.height - point.y
    return ["x": Double(point.x), "y": Double(y)]
  }

  /// Where the drag was last said to be, so a still pointer says nothing.
  private var lastDragPoint: [String: Double] = [:]

  @objc func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
    let count = files(in: sender).count
    guard count > 0 else { return [] }
    // How many and where, for the window's drop overlay to say: where
    // they'd go, or why they can't go anywhere here (app/lib/drop.dart).
    lastDragPoint = dragPoint(sender)
    windowChannel?.invokeMethod(
      "filesOver", arguments: (["count": count] as [String: Any]).merging(lastDragPoint) { a, _ in a })
    return takesFiles ? .copy : []
  }

  @objc func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation {
    let count = files(in: sender).count
    let point = dragPoint(sender)
    if count > 0 && point != lastDragPoint {
      lastDragPoint = point
      windowChannel?.invokeMethod(
        "filesOver", arguments: (["count": count] as [String: Any]).merging(point) { a, _ in a })
    }
    return takesFiles && count > 0 ? .copy : []
  }

  @objc func draggingExited(_ sender: NSDraggingInfo?) {
    windowChannel?.invokeMethod("filesOver", arguments: 0)
  }

  @objc func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
    let paths = files(in: sender)
    guard takesFiles, !paths.isEmpty else {
      windowChannel?.invokeMethod("filesOver", arguments: 0)
      return false
    }
    windowChannel?.invokeMethod(
      "filesDropped", arguments: (["paths": paths] as [String: Any]).merging(dragPoint(sender)) { a, _ in a })
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
/// The share menu for files on this Mac (`shareFiles` on factory/device):
/// `paths`, pointing at `rect` (the Share button or row, in the Flutter
/// view's points from its top left). Answers true once a service shared
/// them, false if the person closed the menu or the service failed (the
/// app deletes the files then), and an "unavailable" error when the menu
/// can't open: no file left, or no window to show it on.
final class ShareFiles: NSObject, NSSharingServicePickerDelegate, NSSharingServiceDelegate {
  /// The share showing now, kept until it answers.
  private static var current: ShareFiles?
  private var result: FlutterResult?

  static func show(_ args: [String: Any], in view: NSView, _ result: @escaping FlutterResult) {
    let urls = (args["paths"] as? [String] ?? []).map { URL(fileURLWithPath: $0) }
      .filter { FileManager.default.fileExists(atPath: $0.path) }
    // Saying why, rather than a false the app would read as the person
    // closing it (Matt's History video, Bruno 44170).
    guard !urls.isEmpty else {
      return result(
        FlutterError(code: "unavailable", message: "the file is no longer there.", details: nil))
    }
    guard view.window != nil else {
      return result(
        FlutterError(
          code: "unavailable", message: "Claktory's window isn't open.", details: nil))
    }
    current?.answer(false)
    let share = ShareFiles()
    share.result = result
    current = share
    var rect = CGRect(x: view.bounds.midX, y: view.bounds.midY, width: 1, height: 1)
    if let r = args["rect"] as? [Double], r.count == 4 {
      let y = view.isFlipped ? r[1] : view.bounds.height - r[1] - r[3]
      rect = CGRect(x: r[0], y: y, width: max(r[2], 1), height: max(r[3], 1))
    }
    let picker = NSSharingServicePicker(items: urls)
    picker.delegate = share
    picker.show(relativeTo: rect, of: view, preferredEdge: .minY)
  }

  private func answer(_ shared: Bool) {
    result?(shared)
    result = nil
    if Self.current === self { Self.current = nil }
  }

  func sharingServicePicker(
    _ picker: NSSharingServicePicker, delegateFor service: NSSharingService
  ) -> NSSharingServiceDelegate? { self }

  func sharingServicePicker(
    _ picker: NSSharingServicePicker, didChoose service: NSSharingService?
  ) {
    // Closed without choosing: nothing was shared.
    if service == nil { answer(false) }
  }

  func sharingService(_ service: NSSharingService, didShareItems items: [Any]) {
    answer(true)
  }

  func sharingService(
    _ service: NSSharingService, didFailToShareItems items: [Any], error: Error
  ) {
    answer(false)
  }
}

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

/// Claktory Dev's watermark (docs/IDENTITY.md): "DEV VERSION" in Geist
/// SemiBold, 120pt with 12pt between letters, in the signal orange at 7%,
/// tilted 0.35 rad up to the right, centred, on two lines when the window is
/// too narrow for one. It never takes an event: AppKit carries on to the
/// views under it (Flutter's, and native ones such as the PDF viewer).
final class DevMarkView: NSView {
  override init(frame: NSRect) {
    super.init(frame: frame)
    wantsLayer = true
    // Over everything the Flutter view composites, platform views included.
    layer?.zPosition = 100_000
  }

  @available(*, unavailable)
  required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

  override func hitTest(_ point: NSPoint) -> NSView? { nil }
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { false }
  override var isOpaque: Bool { false }

  override func viewDidChangeEffectiveAppearance() {
    super.viewDidChangeEffectiveAppearance()
    needsDisplay = true
  }

  /// The app's own Geist SemiBold, from its Flutter assets; the system's
  /// semibold if it can't be read.
  private static let font: NSFont = {
    let size: CGFloat = 120
    if let frameworks = Bundle.main.privateFrameworksURL {
      let url = frameworks.appendingPathComponent(
        "App.framework/Resources/flutter_assets/fonts/Geist-SemiBold.ttf")
      if let data = try? Data(contentsOf: url) as CFData,
        let descriptor = CTFontManagerCreateFontDescriptorFromData(data)
      {
        return CTFontCreateWithFontDescriptor(descriptor, size, nil) as NSFont
      }
    }
    return NSFont.systemFont(ofSize: size, weight: .semibold)
  }()

  /// The palette's signal (app/lib/identity.dart): #FF5B1F dark, #E5480F light.
  private static func signal(dark: Bool) -> NSColor {
    (dark
      ? NSColor(srgbRed: 1, green: 0x5B / 255.0, blue: 0x1F / 255.0, alpha: 1)
      : NSColor(srgbRed: 0xE5 / 255.0, green: 0x48 / 255.0, blue: 0x0F / 255.0, alpha: 1))
      .withAlphaComponent(0.07)
  }

  override func draw(_ dirtyRect: NSRect) {
    let dark = effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
    let paragraph = NSMutableParagraphStyle()
    paragraph.alignment = .center
    // Flutter's height 1.2.
    paragraph.minimumLineHeight = 144
    paragraph.maximumLineHeight = 144
    let text = NSAttributedString(
      string: "DEV VERSION",
      attributes: [
        .font: Self.font,
        .kern: 12,
        .foregroundColor: Self.signal(dark: dark),
        .paragraphStyle: paragraph,
      ])
    // As wide as the window allows, as Flutter lays it out before turning it.
    let width = bounds.width
    let size = text.boundingRect(
      with: NSSize(width: width, height: .greatestFiniteMagnitude),
      options: [.usesLineFragmentOrigin, .usesFontLeading]
    ).size
    guard let context = NSGraphicsContext.current?.cgContext else { return }
    context.saveGState()
    context.translateBy(x: bounds.midX, y: bounds.midY)
    // Flutter's -0.35 turns it counterclockwise on a y-down canvas; AppKit's
    // y runs up, so the same turn is +0.35.
    context.rotate(by: isFlipped ? -0.35 : 0.35)
    text.draw(
      with: NSRect(x: -width / 2, y: -size.height / 2, width: width, height: size.height),
      options: [.usesLineFragmentOrigin, .usesFontLeading])
    context.restoreGState()
  }
}

/// Owns the window's pointer tracking for what's seen: entering the window
/// or moving over it, whichever app is in front.
final class SightPointer: NSResponder {
  init(_ pointed: @escaping () -> Void) {
    self.pointed = pointed
    super.init()
  }

  required init?(coder: NSCoder) { nil }

  private let pointed: () -> Void

  override func mouseEntered(with event: NSEvent) { pointed() }
  override func mouseMoved(with event: NSEvent) { pointed() }
}
