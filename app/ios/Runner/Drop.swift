// Files dragged onto an iPad's window from another app (#512): Files,
// Photos, Safari's pictures, Mail's attachments. They attach as the
// paperclip's do, to the card under the finger that takes files, else the
// card field with the keys, else the message box (app/lib/attachments.dart's
// FileDrop, the Mac window's own contract on `factory/window`):
//
// - `takeFiles` (bool), from the app: whether anything showing takes files
//   now; while it doesn't, a drop of files shows the system's forbidden badge.
// - `takeText` (bool), from the app: whether a message box shows, to take
//   text dragged in (a text selection, a link), which Flutter's own fields
//   don't take on iOS.
// - `filesOver` {count, x, y} while files are over the window (in Flutter's
//   logical points from its view's top left, again whenever they move), and
//   a bare 0 once they leave or the drag ends without a drop.
// - `filesDropping` {kinds, ticket, x, y} at the drop: what's on its way
//   ("photo", "video" or "file" each, as Attach.kind says), so its chips
//   show at once; then `filesDropped` {paths, ticket, x, y} once every copy
//   is made, in a folder of its own under tmp/Attachments (none for an item
//   that couldn't be copied).
// - `textDropped` {text, x, y}: text dropped where the app takes it.
//
// iPad only: an iPhone takes no drops from other apps here.

import Flutter
import UIKit
import UniformTypeIdentifiers

final class Drop: NSObject, FlutterSceneLifeCycleDelegate, UIDropInteractionDelegate {
  static let shared = Drop()

  private var channel: FlutterMethodChannel?
  private(set) var takesFiles = false
  private(set) var takesText = false
  private var nextTicket = 1
  private weak var view: UIView?
  private var interaction: UIDropInteraction?

  private var lastPoint: CGPoint?

  func register(with messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: "factory/window", binaryMessenger: messenger)
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else { return result(nil) }
      switch call.method {
      case "takeFiles":
        self.takesFiles = call.arguments as? Bool ?? false
        result(nil)
      case "takeText":
        self.takesText = call.arguments as? Bool ?? false
        result(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
    self.channel = channel
  }

  func sceneDidBecomeActive(_ scene: UIScene) {
    attach(to: scene)
  }

  func sceneWillEnterForeground(_ scene: UIScene) {
    attach(to: scene)
  }

  private func attach(to scene: UIScene) {
    guard UIDevice.current.userInterfaceIdiom == .pad,
      let root = (scene as? UIWindowScene)?.windows.first(where: \.isKeyWindow)?
        .rootViewController
        ?? (scene as? UIWindowScene)?.windows.first?.rootViewController,
      let view = root.view, view !== self.view
    else { return }
    if let interaction, let old = self.view { old.removeInteraction(interaction) }
    let interaction = UIDropInteraction(delegate: self)
    view.addInteraction(interaction)
    self.view = view
    self.interaction = interaction
  }

  /// Text alone (a selection, a link) goes in as text; Files and Photos
  /// name what they give.
  static func isFile(types: [String], named: Bool) -> Bool {
    let kinds = types.compactMap(UTType.init)
    if kinds.contains(where: { $0.conforms(to: .image) || $0.conforms(to: .audiovisualContent) }) {
      return true
    }
    if named { return true }
    return kinds.contains { !$0.conforms(to: .text) && !$0.conforms(to: .url) }
  }

  /// JPEG or PNG before HEIC: agents read them more readily.
  static func copyType(of types: [String]) -> String? {
    types.first { $0 == UTType.jpeg.identifier }
      ?? types.first { $0 == UTType.png.identifier }
      ?? types.first
  }

  static func fileName(suggested: String?, type: String, photo: Bool) -> String {
    let base = suggested.flatMap { $0.isEmpty ? nil : $0 } ?? (photo ? "Photo" : "File")
    // Never a path: only its last part.
    let name = (base as NSString).lastPathComponent
    guard let copied = UTType(type), let ext = copied.preferredFilenameExtension else {
      return name
    }
    // Its own extension when it already says what it's copied as.
    let has = (name as NSString).pathExtension
    if !has.isEmpty, let own = UTType(filenameExtension: has), own.conforms(to: copied) {
      return name
    }
    return "\(name).\(ext)"
  }

  private func providers(_ session: UIDropSession) -> [NSItemProvider] {
    session.items.map(\.itemProvider).filter {
      Self.isFile(types: $0.registeredTypeIdentifiers, named: $0.suggestedName != nil)
    }
  }

  private func hasText(_ session: UIDropSession) -> Bool {
    session.canLoadObjects(ofClass: String.self) || session.canLoadObjects(ofClass: URL.self)
  }

  private func point(_ session: UIDropSession) -> [String: Double] {
    guard let view else { return [:] }
    let point = session.location(in: view)
    return ["x": Double(point.x), "y": Double(point.y)]
  }

  func dropInteraction(_ interaction: UIDropInteraction, canHandle session: UIDropSession) -> Bool {
    // Not a drag from inside the app (it has none of its own).
    session.localDragSession == nil && (!providers(session).isEmpty || hasText(session))
  }

  func dropInteraction(
    _ interaction: UIDropInteraction, sessionDidUpdate session: UIDropSession
  ) -> UIDropProposal {
    let count = providers(session).count
    guard count > 0 else {
      return UIDropProposal(operation: takesText && hasText(session) ? .copy : .forbidden)
    }
    if let view {
      let now = session.location(in: view)
      if now != lastPoint {
        lastPoint = now
        channel?.invokeMethod(
          "filesOver",
          arguments: (["count": count] as [String: Any]).merging(point(session)) { a, _ in a })
      }
    }
    return UIDropProposal(operation: takesFiles ? .copy : .forbidden)
  }

  func dropInteraction(_ interaction: UIDropInteraction, sessionDidExit session: UIDropSession) {
    left()
  }

  func dropInteraction(_ interaction: UIDropInteraction, sessionDidEnd session: UIDropSession) {
    left()
  }

  private func left() {
    guard lastPoint != nil else { return }
    lastPoint = nil
    channel?.invokeMethod("filesOver", arguments: 0)
  }

  func dropInteraction(_ interaction: UIDropInteraction, performDrop session: UIDropSession) {
    let at = point(session)
    let files = providers(session)
    // The overlay goes: what's dropped shows where it went.
    left()
    guard !files.isEmpty else {
      guard takesText else { return }
      if session.canLoadObjects(ofClass: String.self) {
        _ = session.loadObjects(ofClass: String.self) { [weak self] items in
          self?.dropped(text: items.joined(separator: "\n"), at: at)
        }
      } else {
        _ = session.loadObjects(ofClass: URL.self) { [weak self] items in
          self?.dropped(text: items.map(\.absoluteString).joined(separator: "\n"), at: at)
        }
      }
      return
    }
    guard takesFiles else { return }
    let ticket = nextTicket
    nextTicket += 1
    channel?.invokeMethod(
      "filesDropping",
      arguments: (["kinds": files.map(Attach.kind(of:)), "ticket": ticket] as [String: Any])
        .merging(at) { a, _ in a })
    copy(files) { [weak self] paths in
      self?.channel?.invokeMethod(
        "filesDropped",
        arguments: (["paths": paths, "ticket": ticket] as [String: Any]).merging(at) { a, _ in a })
    }
  }

  private func dropped(text: String, at: [String: Double]) {
    guard !text.isEmpty else { return }
    channel?.invokeMethod(
      "textDropped", arguments: (["text": text] as [String: Any]).merging(at) { a, _ in a })
  }

  func copy(_ files: [NSItemProvider], then done: @escaping ([String]) -> Void) {
    let group = DispatchGroup()
    let lock = NSLock()
    var paths = [Int: String]()
    for (index, provider) in files.enumerated() {
      guard let type = Self.copyType(of: provider.registeredTypeIdentifiers) else { continue }
      let name = Self.fileName(
        suggested: provider.suggestedName, type: type, photo: Attach.kind(of: provider) == "photo")
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
    group.notify(queue: .main) { done(paths.keys.sorted().compactMap { paths[$0] }) }
  }

  static func copy(_ url: URL, as name: String) -> String? {
    let folder = FileManager.default.temporaryDirectory
      .appendingPathComponent("Attachments").appendingPathComponent(UUID().uuidString)
    let file = folder.appendingPathComponent(name)
    do {
      try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
      try FileManager.default.copyItem(at: url, to: file)
      return file.path
    } catch {
      return nil
    }
  }
}
