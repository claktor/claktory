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
    let launched = super.application(application, didFinishLaunchingWithOptions: launchOptions)
    // Before launching finishes, so a tap that opened the app reaches it
    // (Notifications.swift).
    Notifications.shared.launch()
    return launched
  }

  // This iPhone's push token, for the push relay (Notifications.swift).
  override func application(
    _ application: UIApplication,
    didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data
  ) {
    super.application(application, didRegisterForRemoteNotificationsWithDeviceToken: deviceToken)
    Notifications.shared.received(token: deviceToken)
  }

  func didInitializeImplicitFlutterEngine(_ engineBridge: FlutterImplicitEngineBridge) {
    GeneratedPluginRegistrant.register(with: engineBridge.pluginRegistry)
    let messenger = engineBridge.pluginRegistry.registrar(forPlugin: "Device")!.messenger()
    // The camera's picture that scans pairing codes (Scanner.swift).
    let scanner = engineBridge.pluginRegistry.registrar(forPlugin: "Scanner")!
    scanner.register(ScannerFactory(messenger: scanner.messenger()), withId: "claktory/scanner")
    // The video viewer's player (app/macos/Runner/VideoPlayers.swift).
    VideoPlayers.register(with: engineBridge.pluginRegistry.registrar(forPlugin: "VideoPlayers")!)
    // Voice messages: recording them and playing them back, as on the Mac
    // (app/macos/Runner/Recorder.swift, AudioPlayer.swift).
    Recorder.register(with: messenger)
    AudioPlayer.register(with: messenger)
    // History's video export, its frames into an MP4, as on the Mac
    // (app/macos/Runner/ReplayWriter.swift).
    ReplayWriter.register(with: messenger)
    // The network path, to reconnect the moment it changes, as on the Mac
    // (app/macos/Runner/NetworkPath.swift).
    NetworkPath.register(with: messenger)
    // A short grace after leaving the app, so a quick trip to another app
    // keeps the connections (BackgroundGrace.swift).
    BackgroundGrace.register(with: messenger)
    // A website a friend shares over a pipe, opened in Safari's view over
    // the app, which keeps its forward up (ShareBrowser.swift, W6).
    ShareBrowser.register(with: messenger)
    // Copying a secret, such as a device's add code, marked sensitive and
    // expiring (app/macos/Runner/SecretClipboard.swift).
    SecretClipboard.register(with: messenger)
    // The keys of end-to-end direct messages, in the app's own keychain
    // group (app/macos/Runner/DmKeys.swift).
    DmKeys.register(with: messenger)
    // A hardware keyboard coming and going (Capabilities.swift).
    KeyboardEvents.register(with: messenger)
    // An iPad window's controls kept clear of the app (Capabilities.swift).
    WindowControls.start(with: messenger)
    // The iPad workbench's commands in the system's menu (CommandMenu.swift).
    CommandMenu.register(with: messenger)
    // Documents and ink (Ink/InkChannel.swift).
    Ink.register(with: engineBridge.pluginRegistry)
    // Links that open the app: claktory://add?code=…, and a pairing
    // code's factory://… from the Camera (Links, below).
    let links = engineBridge.pluginRegistry.registrar(forPlugin: "Links")!
    links.addSceneDelegate(Links.shared)
    Links.shared.register(with: links.messenger())
    // The Home Screen's quick action, Needs you (QuickActions, below).
    let quick = engineBridge.pluginRegistry.registrar(forPlugin: "QuickActions")!
    quick.addSceneDelegate(QuickActions.shared)
    QuickActions.shared.register(with: quick.messenger())
    // Notifications: those the app shows, its pushes' taps, and this
    // iPhone's registration for pushes (Notifications.swift, Push.swift).
    Notifications.shared.register(with: messenger)
    deviceChannel = FlutterMethodChannel(name: "factory/device", binaryMessenger: messenger)
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
      case "qrCode":
        result(Self.qrCode(call.arguments as? String ?? ""))
      case "cameraAccess":
        result(Camera.access())
      case "requestCamera":
        Camera.request { result($0) }
      case "protectKeptFiles":
        // #446 (Fugo 33088, 33120): the files kept from a Claktory on
        // another machine can't be opened while the iPhone is locked.
        guard let path = call.arguments as? String else {
          result(FlutterError(code: "bad_arguments", message: "a path", details: nil))
          return
        }
        DispatchQueue.global(qos: .utility).async {
          let failure: FlutterError?
          do {
            try Self.protectKeptFiles(path)
            failure = nil
          } catch {
            failure = FlutterError(code: "protect_failed", message: "\(error)", details: nil)
          }
          DispatchQueue.main.async { result(failure) }
        }
      case "openCameraSettings":
        Camera.openSettings()
        result(nil)
      case "pickAttachments":
        Attach.shared.pick(call.arguments as? [String: Any] ?? [:], result)
      case "takePhoto":
        Attach.shared.takePhoto(result)
      case "pickedAttachments":
        // A staged pick's copies, once they're done (Attach.picked).
        Attach.shared.picked(call.arguments as? [String: Any] ?? [:], result)
      case "webAuth":
        WebAuth.shared.start(call.arguments, result)
      case "cancelWebAuth":
        WebAuth.shared.cancel()
        result(nil)
      case "share":
        // The system's share sheet for text (lib/mobile/share.dart).
        Self.share(call.arguments as? [String: Any] ?? [:], result)
      case "shareFiles":
        // The share sheet for files the app fetched (lib/share_files.dart,
        // #418): AirDrop, Messages, Save to Files…
        Self.shareFiles(call.arguments as? [String: Any] ?? [:], result)
      case "localNetwork":
        // Whether the local network may be reached (LocalNetwork.swift):
        // asks the person first when they haven't answered.
        LocalNetworkCheck.check { result($0) }
      case "openAppSettings":
        LocalNetworkCheck.openSettings()
        result(nil)
      case "capabilities":
        // What this device is (Capabilities.swift): the iPad's workbench
        // follows its idiom, never its window's size.
        result(Capabilities.current())
      case "timeZone":
        // The device's IANA zone ("Europe/London"), which Flutter can't
        // read: Claktory's browser takes it from the device that first
        // opens it (#435).
        result(TimeZone.current.identifier)
      default: result(FlutterMethodNotImplemented)
      }
    }
  }

  // The workbench's commands (CommandMenu.swift): at the end of the View
  // menu and in the list shown while ⌘ is held. The app delegate ends the
  // responder chain, so they're found wherever the focus is; a disabled one
  // has nothing to perform it, so its keys go on to what's focused.
  override func buildMenu(with builder: UIMenuBuilder) {
    super.buildMenu(with: builder)
    CommandMenu.shared.build(builder, action: #selector(runWorkbenchCommand(_:)))
  }

  override func canPerformAction(_ action: Selector, withSender sender: Any?) -> Bool {
    if action == #selector(runWorkbenchCommand(_:)) {
      return CommandMenu.shared.isEnabled((sender as? UICommand)?.propertyList as? String)
    }
    return super.canPerformAction(action, withSender: sender)
  }

  @objc func runWorkbenchCommand(_ sender: UICommand) {
    guard let id = sender.propertyList as? String else { return }
    CommandMenu.shared.run(id)
  }

  // What the app keeps on the device (app/lib/drafts.dart, connections.dart),
  // as on the Mac (MainFlutterWindow.swift): unsent drafts by agent id, and
  // which Factory it shows with this device's key for each.
  private var deviceChannel: FlutterMethodChannel?
  private static let draftsKey = "Drafts"
  /// In the keychain (app/macos/Runner/ConnectionStore.swift, the Mac's).
  private static let connections = ConnectionStore.forThisApp()

  /// The share sheet for `text`, over whatever shows now. On an iPad it
  /// points at `rect` (the Share button, in the Flutter view's points),
  /// or sits in the middle without an arrow. True once the person sent it
  /// somewhere; false if they closed it, or nothing could show it.
  private static func share(_ args: [String: Any], _ result: @escaping FlutterResult) {
    guard let text = args["text"] as? String, !text.isEmpty else {
      result(false)
      return
    }
    present([text], args, result)
  }

  /// The share sheet for `paths`, files on this device (the app deletes
  /// them once it answers). On an iPad it points at `rect`, as for text.
  /// True once the person sent them somewhere.
  private static func shareFiles(_ args: [String: Any], _ result: @escaping FlutterResult) {
    let urls = (args["paths"] as? [String] ?? []).map { URL(fileURLWithPath: $0) }
      .filter { FileManager.default.fileExists(atPath: $0.path) }
    guard !urls.isEmpty else {
      result(false)
      return
    }
    present(urls, args, result)
  }

  private static func present(_ items: [Any], _ args: [String: Any], _ result: @escaping FlutterResult) {
    let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
    let scene = scenes.first { $0.activationState == .foregroundActive } ?? scenes.first
    guard let window = scene?.keyWindow ?? scene?.windows.first,
      var top = window.rootViewController
    else {
      result(false)
      return
    }
    while let shown = top.presentedViewController { top = shown }
    let sheet = UIActivityViewController(activityItems: items, applicationActivities: nil)
    if let popover = sheet.popoverPresentationController {
      popover.sourceView = top.view
      if let r = args["rect"] as? [Double], r.count == 4 {
        popover.sourceRect = CGRect(x: r[0], y: r[1], width: r[2], height: r[3])
      } else {
        popover.sourceRect = CGRect(x: top.view.bounds.midX, y: top.view.bounds.midY, width: 0, height: 0)
        popover.permittedArrowDirections = []
      }
    }
    // Answered once: the sheet can report again (an activity's own screen
    // closing back to it), and a second answer would end the app.
    var answered = false
    sheet.completionWithItemsHandler = { _, completed, _, _ in
      guard !answered else { return }
      answered = true
      result(completed)
    }
    top.present(sheet, animated: true)
  }

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

/// Where the person attaches files from (app/lib/attachments.dart): the
/// system's menu (Choose from Photos, Choose from Files, and Paste image when the
/// clipboard holds a picture), then its picker. What's chosen is copied
/// into this app's temporary folder, and the copies' paths answered.
///
/// Staged (`staged`: the message box's and a card's paperclip), a photo
/// pick answers as soon as the picker closes with what's on its way,
/// `{"pending": ["photo", "video", …], "ticket": n}`, for the app to show
/// at once: copying a big photo, or one still in iCloud, takes seconds.
/// `pickedAttachments` with that ticket answers the paths once every copy
/// is done. Anything else answers the paths themselves, as before.
final class Attach: NSObject, PHPickerViewControllerDelegate, UIDocumentPickerDelegate,
  UIImagePickerControllerDelegate, UINavigationControllerDelegate
{
  static let shared = Attach()

  private var result: FlutterResult?

  /// The pick under way answers what's on its way first.
  private var staged = false

  /// Staged picks' copies, by ticket: their paths once done, or the
  /// answer waiting for them.
  private var copying = [Int: (paths: [String]?, waiting: FlutterResult?)]()
  private var nextTicket = 0

  /// `rect`, the paperclip's [left, top, width, height] in the window, is
  /// what an iPad's menu points at; without it the menu shows in the middle,
  /// with no arrow. An iPad's action sheet is a popover, and UIKit ends the
  /// app if one has nothing to come from (Zucchero row 630).
  func pick(_ args: [String: Any], _ result: @escaping FlutterResult) {
    guard self.result == nil, let presenter = Self.presenter() else { return result([]) }
    self.result = result
    staged = args["staged"] as? Bool ?? false
    let menu = UIAlertController(title: nil, message: nil, preferredStyle: .actionSheet)
    // The rect is in the Flutter view's points, so the window's root view
    // is what it's measured in, whatever is presented over it (Sorbet 45406).
    let flutterView = presenter.view.window?.rootViewController?.view ?? presenter.view!
    Self.anchor(menu, in: flutterView, rect: args["rect"] as? [Double])
    // First, as Apple's own apps have it; only where the caller takes a
    // photo (`camera`) and this device has a camera. Chosen, the menu
    // closes and the app explains the camera before it opens (takePhoto).
    if args["camera"] as? Bool == true, Self.hasCamera {
      menu.addAction(
        UIAlertAction(title: "Take Photo", style: .default) { _ in
          self.fail("take_photo", "Take Photo was chosen.")
        })
    }
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

  /// Points [menu]'s popover (an iPad's) at [rect] in [view], or at the
  /// middle of [view] with no arrow when there's no rect.
  static func anchor(_ menu: UIAlertController, in view: UIView, rect: [Double]?) {
    guard let popover = menu.popoverPresentationController else { return }
    popover.sourceView = view
    if let r = rect, r.count == 4 {
      popover.sourceRect = CGRect(x: r[0], y: r[1], width: r[2], height: r[3])
    } else {
      popover.sourceRect = CGRect(x: view.bounds.midX, y: view.bounds.midY, width: 0, height: 0)
      popover.permittedArrowDirections = []
    }
  }

  private func finish(_ paths: [String]) {
    result?(paths)
    result = nil
  }

  private func fail(_ code: String, _ message: String) {
    result?(FlutterError(code: code, message: message, details: nil))
    result = nil
  }

  /// A camera to take a photo with; in a build other than the App Store's,
  /// the camera's pretend switch (Camera.Pretend) stands in for one, so QA
  /// reaches Take Photo in the Simulator.
  static var hasCamera: Bool {
    UIImagePickerController.isSourceTypeAvailable(.camera) || Camera.Pretend.current() != nil
  }

  /// Takes a photo with the camera, full screen, once the app has
  /// explained it and the person allowed it: its path, a JPEG named for
  /// when it was taken ("Photo 2026-11-26 at 21.04.33.jpg"), or [] when
  /// cancelled. camera_denied without the camera's permission (the app asks
  /// first), camera_unavailable without a camera.
  func takePhoto(_ result: @escaping FlutterResult) {
    guard self.result == nil, let presenter = Self.presenter() else { return result([]) }
    guard Self.hasCamera else {
      return result(
        FlutterError(code: "camera_unavailable", message: "This device has no camera.", details: nil))
    }
    guard Camera.access() == "granted" else {
      return result(
        FlutterError(code: "camera_denied", message: "Claktory may not use the camera.", details: nil))
    }
    self.result = result
    guard UIImagePickerController.isSourceTypeAvailable(.camera) else {
      // The pretend camera (QA, in the Simulator): a picture made here.
      return save(Self.pretendPhoto())
    }
    let camera = UIImagePickerController()
    camera.sourceType = .camera
    camera.cameraCaptureMode = .photo
    camera.delegate = self
    camera.modalPresentationStyle = .fullScreen
    presenter.present(camera, animated: true)
  }

  func imagePickerController(
    _ picker: UIImagePickerController,
    didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]
  ) {
    picker.dismiss(animated: true)
    guard let image = info[.originalImage] as? UIImage else { return finish([]) }
    save(image)
  }

  func imagePickerControllerDidCancel(_ picker: UIImagePickerController) {
    picker.dismiss(animated: true)
    finish([])
  }

  /// [image] as a JPEG in a folder of its own, off the main thread: written
  /// from the picture itself, so it carries no location.
  private func save(_ image: UIImage) {
    let name = "Photo \(Self.photoTime.string(from: Date())).jpg"
    DispatchQueue.global(qos: .userInitiated).async {
      var failure: (String, String)?
      var path: String?
      if let data = image.jpegData(compressionQuality: 0.9), let folder = Self.folder() {
        let file = folder.appendingPathComponent(name)
        do {
          try data.write(to: file)
          path = file.path
        } catch let error as NSError
          where error.domain == NSCocoaErrorDomain && error.code == NSFileWriteOutOfSpaceError
        {
          failure = ("no_space", "This device hasn't room for the photo.")
        } catch {
          failure = ("copy_failed", "The photo couldn't be kept (\(error)).")
        }
      } else {
        failure = ("copy_failed", "The photo couldn't be made into a JPEG.")
      }
      DispatchQueue.main.async {
        if let path { self.finish([path]) } else if let failure { self.fail(failure.0, failure.1) }
      }
    }
  }

  /// "2026-11-26 at 21.04.33", local time, as a Mac names a screenshot.
  static let photoTime: DateFormatter = {
    let format = DateFormatter()
    format.locale = Locale(identifier: "en_US_POSIX")
    format.dateFormat = "yyyy-MM-dd 'at' HH.mm.ss"
    return format
  }()

  /// What the pretend camera "takes": a plain picture saying so.
  private static func pretendPhoto() -> UIImage {
    let size = CGSize(width: 1200, height: 900)
    return UIGraphicsImageRenderer(size: size).image { context in
      UIColor.systemTeal.setFill()
      context.fill(CGRect(origin: .zero, size: size))
      let words = "Pretend photo" as NSString
      words.draw(
        at: CGPoint(x: 80, y: 400),
        withAttributes: [.font: UIFont.boldSystemFont(ofSize: 96), .foregroundColor: UIColor.white])
    }
  }

  private func photos(_ presenter: UIViewController) {
    var configuration = PHPickerConfiguration()
    configuration.selectionLimit = 0
    let picker = PHPickerViewController(configuration: configuration)
    picker.delegate = self
    presenter.present(picker, animated: true)
  }

  /// What a staged pick's copies came to: their paths, as soon as every
  /// copy is done; none for a ticket that isn't one.
  func picked(_ args: [String: Any], _ result: @escaping FlutterResult) {
    guard let ticket = args["ticket"] as? Int, let entry = copying[ticket] else {
      return result([String]())
    }
    if let paths = entry.paths {
      copying[ticket] = nil
      result(paths)
    } else {
      copying[ticket] = (nil, result)
    }
  }

  /// What a chosen item is, for the app to say what's on its way.
  static func kind(of provider: NSItemProvider) -> String {
    if provider.hasItemConformingToTypeIdentifier(UTType.movie.identifier) { return "video" }
    if provider.hasItemConformingToTypeIdentifier(UTType.image.identifier) { return "photo" }
    return "file"
  }

  func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
    picker.dismiss(animated: true)
    let group = DispatchGroup()
    let lock = NSLock()
    var paths = [Int: String]()
    var kinds = [String]()
    for (index, item) in results.enumerated() {
      let provider = item.itemProvider
      // JPEG when the photo offers it: agents read it more readily than HEIC.
      let types = provider.registeredTypeIdentifiers
      guard
        let type = types.first(where: { $0 == UTType.jpeg.identifier })
          ?? types.first(where: { $0 == UTType.png.identifier }) ?? types.first
      else { continue }
      kinds.append(Self.kind(of: provider))
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
    func copied() -> [String] { paths.keys.sorted().compactMap { paths[$0] } }
    guard staged, !kinds.isEmpty else {
      return group.notify(queue: .main) { self.finish(copied()) }
    }
    // What's on its way shows now; the paths follow (picked).
    let ticket = nextTicket
    nextTicket += 1
    copying[ticket] = (nil, nil)
    result?(["pending": kinds, "ticket": ticket] as [String: Any])
    result = nil
    group.notify(queue: .main) {
      let done = copied()
      if let waiting = self.copying[ticket]?.waiting {
        self.copying[ticket] = nil
        waiting(done)
      } else {
        self.copying[ticket] = (done, nil)
      }
    }
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

/// Links that open the app, as on the Mac (links.dart): `claktory://add?code=…`
/// (the build's own scheme), and a pairing code's own `factory://…`, which
/// the Camera hands over when it reads a Claktory's QR code. Kept until the
/// app listens on `factory/links`, then each as it comes. The app asks
/// before connecting to anything a link names; a link alone does nothing.
final class Links: NSObject, FlutterSceneLifeCycleDelegate {
  static let shared = Links()

  /// The schemes a link may have; anything else is dropped here.
  static let schemes: Set<String> = ["factory", "claktory", "claktory-dev", "claktory-debug"]

  /// Longer than any pairing code: dropped before the app parses it.
  static let maxLength = 4096

  private var channel: FlutterMethodChannel?
  private var listening = false
  private(set) var waiting: [String] = []

  func register(with messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: "factory/links", binaryMessenger: messenger)
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else { return result(nil) }
      switch call.method {
      case "listen":
        self.listening = true
        result(self.waiting)
        self.waiting = []
      default:
        result(FlutterMethodNotImplemented)
      }
    }
    self.channel = channel
  }

  /// [urls] opened the app: those it takes go to the app, or wait for it.
  /// Never logged: a pairing code is a credential.
  @discardableResult
  func open(_ urls: [URL]) -> Bool {
    var took = false
    for url in urls {
      let link = url.absoluteString
      guard let scheme = url.scheme?.lowercased(), Self.schemes.contains(scheme),
        link.utf8.count <= Self.maxLength
      else { continue }
      took = true
      if listening, let channel {
        channel.invokeMethod("opened", arguments: link)
      } else {
        waiting.append(link)
      }
    }
    return took
  }

  // The app opened by a link: it comes with the scene's connection.
  func scene(
    _ scene: UIScene, willConnectTo session: UISceneSession,
    options connectionOptions: UIScene.ConnectionOptions?
  ) -> Bool {
    open(connectionOptions?.urlContexts.map(\.url) ?? [])
  }

  // A link while the app runs (or wakes from the background).
  func scene(_ scene: UIScene, openURLContexts URLContexts: Set<UIOpenURLContext>) -> Bool {
    open(URLContexts.map(\.url))
  }
}

/// The Home Screen's quick action (touch and hold the app's icon): Needs
/// you, with how many wait as its subtitle (#514 §7.2). Its words come
/// from the app ("set"), which keeps them current while it runs; a tap
/// opens Needs you ("open"), or waits for the app to listen when it
/// launched the app.
final class QuickActions: NSObject, FlutterSceneLifeCycleDelegate {
  static let shared = QuickActions()

  /// The one quick action's type.
  static let needsYou = "needs-you"

  private var channel: FlutterMethodChannel?
  private var listening = false
  private var waiting = false

  func register(with messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: "factory/quick_actions", binaryMessenger: messenger)
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else { return result(nil) }
      switch call.method {
      case "listen":
        // Whether a tap opened the app before it listened.
        self.listening = true
        result(self.waiting)
        self.waiting = false
      case "set":
        let words = call.arguments as? [String: Any] ?? [:]
        let title = words["title"] as? String ?? ""
        let subtitle = words["subtitle"] as? String ?? ""
        UIApplication.shared.shortcutItems =
          title.isEmpty
          ? []
          : [
            UIApplicationShortcutItem(
              type: Self.needsYou, localizedTitle: title,
              localizedSubtitle: subtitle.isEmpty ? nil : subtitle,
              icon: UIApplicationShortcutIcon(systemImageName: "tray"), userInfo: nil)
          ]
        result(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
    self.channel = channel
  }

  @discardableResult
  func take(_ item: UIApplicationShortcutItem) -> Bool {
    guard item.type == Self.needsYou else { return false }
    if listening, let channel {
      channel.invokeMethod("open", arguments: item.type)
    } else {
      waiting = true
    }
    return true
  }

  // The app launched by the quick action.
  func scene(
    _ scene: UIScene, willConnectTo session: UISceneSession,
    options connectionOptions: UIScene.ConnectionOptions?
  ) -> Bool {
    guard let item = connectionOptions?.shortcutItem else { return false }
    return take(item)
  }

  // The quick action while the app runs, or wakes from the background.
  func windowScene(
    _ windowScene: UIWindowScene, performActionFor shortcutItem: UIApplicationShortcutItem,
    completionHandler: @escaping (Bool) -> Void
  ) -> Bool {
    let took = take(shortcutItem)
    completionHandler(took)
    return took
  }
}

// The files kept from a Claktory on another machine (#446,
// app/lib/viewers/file_cache.dart).
extension AppDelegate {
  /// Unreadable while the iPhone is locked, though a write in progress
  /// finishes (Fugo 33120). What's made in a folder later takes its class.
  static let keptFilesProtection = FileProtectionType.completeUnlessOpen

  /// A folder (made if it isn't there) and everything in it, or one file
  /// moved in, which keeps the class it was made with.
  static func protectKeptFiles(_ path: String) throws {
    let manager = FileManager.default
    let protection: [FileAttributeKey: Any] = [.protectionKey: keptFilesProtection]
    var folder: ObjCBool = false
    if manager.fileExists(atPath: path, isDirectory: &folder), !folder.boolValue {
      try manager.setAttributes(protection, ofItemAtPath: path)
      return
    }
    try manager.createDirectory(
      atPath: path, withIntermediateDirectories: true, attributes: protection)
    try manager.setAttributes(protection, ofItemAtPath: path)
    // What an earlier version kept there, before the folder had it.
    if let items = manager.enumerator(atPath: path) {
      for case let item as String in items {
        try manager.setAttributes(
          protection, ofItemAtPath: (path as NSString).appendingPathComponent(item))
      }
    }
  }
}
