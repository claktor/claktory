// The Mac's PDF viewer (pane kind "pdf"): PDFKit's own view in the pane,
// so pages scroll and zoom with the trackpad and their text is selected
// and copied as anywhere on the Mac. Only a viewer: no drawing, forms only
// shown, links never opened here (Dart offers them), no scripts.
//
// The same channels and messages as the iPad's ink surfaces
// (app/ios/Runner/Ink, fixtures in app/test/ipad/fixtures/bridge), less
// the ink: platform view "claktory/ink" (app/lib/ipad/ink_surface.dart's
// AppKitView on a Mac), one channel per view "claktory/ink/<id>" with
// attach, goToPage, search; ready, pageChanged, linkTapped, error. Mac
// only: findRequested {forward}, when ⌘F (⌘G, ⇧⌘G) is pressed with the
// page focused, for the app's find bar. The app-wide "claktory/ink" channel
// is the render cache (PaperCache.swift). And loadDrawing, as on the iPad:
// ink saved there shows over its page, only read (#368, [InkOverlay]).

import Cocoa
import FlutterMacOS
import PDFKit
import PencilKit

enum Paper {
  static let viewType = "claktory/ink"

  static func register(with registrar: FlutterPluginRegistrar) {
    let messenger = registrar.messenger
    registrar.register(PaperFactory(messenger: messenger), withId: viewType)
    let channel = FlutterMethodChannel(name: viewType, binaryMessenger: messenger)
    channel.setMethodCallHandler { call, result in cache(call, result) }
    appChannel = channel
  }

  private static var appChannel: FlutterMethodChannel?

  /// The render cache on the app's channel, as InkRenderCache on the iPad.
  static func cache(_ call: FlutterMethodCall, _ result: @escaping FlutterResult) {
    let cache = PaperCache.shared
    let args = call.arguments as? [String: Any] ?? [:]
    func bad(_ what: String) -> FlutterError {
      FlutterError(code: "bad_request", message: what, details: nil)
    }
    func file() -> URL? {
      cache.file(
        backend: args["backend"] as? String, sha256: args["sha256"] as? String,
        name: args["name"] as? String)
    }
    switch call.method {
    case "cacheBegin":
      guard let url = file(), let sha = args["sha256"] as? String else {
        return result(bad("backend, sha256 and name"))
      }
      do { result(["token": try cache.begin(url, sha256: sha)]) } catch {
        result(FlutterError(code: "io", message: error.localizedDescription, details: nil))
      }
    case "cacheAppend":
      guard let token = args["token"] as? String, let bytes = args["bytes"] as? FlutterStandardTypedData,
        cache.append(token, bytes.data)
      else { return result(bad("token and bytes")) }
      result(["written": bytes.data.count])
    case "cacheFinish":
      guard let token = args["token"] as? String else { return result(bad("token")) }
      cache.finish(token) { outcome in
        switch outcome {
        case .success(let url): result(["file": url.path])
        case .failure(let error):
          result(FlutterError(code: error.code, message: error.message, details: error.details))
        }
      }
    case "cacheCancel":
      if let token = args["token"] as? String { cache.cancel(token) }
      result(nil)
    case "cacheLookup":
      guard let url = file() else { return result(bad("backend, sha256 and name")) }
      result(cache.lookup(url).map { ["file": $0.path] })
    case "cacheClear":
      cache.clear(backend: args["backend"] as? String)
      result(nil)
    default:
      result(FlutterMethodNotImplemented)
    }
  }
}

final class PaperFactory: NSObject, FlutterPlatformViewFactory {
  private let messenger: FlutterBinaryMessenger

  init(messenger: FlutterBinaryMessenger) {
    self.messenger = messenger
  }

  func createArgsCodec() -> (FlutterMessageCodec & NSObjectProtocol)? {
    FlutterStandardMessageCodec.sharedInstance()
  }

  func create(withViewIdentifier viewId: Int64, arguments args: Any?) -> NSView {
    let channel = FlutterMethodChannel(
      name: "\(Paper.viewType)/\(viewId)", binaryMessenger: messenger)
    return PaperPane(params: args as? [String: Any] ?? [:], channel: channel)
  }
}

/// One viewer: the PDFView, its channel and the document once opened.
final class PaperPane: NSView, PDFViewDelegate {
  let pdf = PaperPDFView()
  private let channel: FlutterMethodChannel
  private let params: [String: Any]
  private var attached = false
  private var queued: [(String, Any?)] = []
  private var notes: [NSObjectProtocol] = []
  private var url: URL?
  private let searching = DispatchQueue(label: "claktory.paper.search", qos: .userInitiated)
  private var searches = 0
  /// Each page's saved ink (loadDrawing), laid over it once the document
  /// is open.
  private var inks: [Int: PKDrawing] = [:]
  private var overlays: [Int: InkOverlay] = [:]

  init(params: [String: Any], channel: FlutterMethodChannel) {
    self.params = params
    self.channel = channel
    super.init(frame: .zero)
    pdf.frame = bounds
    pdf.autoresizingMask = [.width, .height]
    pdf.displayMode = .singlePageContinuous
    pdf.displayDirection = .vertical
    pdf.autoScales = true
    pdf.backgroundColor = NSColor(white: 0.93, alpha: 1)
    pdf.delegate = self
    pdf.onFind = { [weak self] forward in
      self?.send("findRequested", ["forward": forward.map { $0 as Any } ?? NSNull()])
    }
    addSubview(pdf)
    notes.append(
      NotificationCenter.default.addObserver(
        forName: .PDFViewPageChanged, object: pdf, queue: .main
      ) { [weak self] _ in self?.pageChanged() })
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else { return result(nil) }
      self.handle(call, result)
    }
    open()
  }

  required init?(coder: NSCoder) { nil }

  deinit {
    notes.forEach(NotificationCenter.default.removeObserver)
    channel.setMethodCallHandler(nil)
  }

  // MARK: Events

  /// Sent once Dart has attached; until then they wait.
  private func send(_ method: String, _ arguments: Any?) {
    if attached {
      channel.invokeMethod(method, arguments: arguments)
    } else {
      queued.append((method, arguments))
    }
  }

  // MARK: Opening

  /// Opens the cached copy once its hash is the version the backend named.
  private func open() {
    let document = params["document"] as? [String: Any] ?? [:]
    let source = params["source"] as? [String: Any] ?? [:]
    guard let path = document["file"] as? String else {
      return send("error", ["code": "document_unreadable", "recoverable": false])
    }
    let expected = document["sha256"] as? String ?? source["sha256"] as? String
    DispatchQueue.global(qos: .userInitiated).async { [weak self] in
      let opened = PaperCore.open(path: path, expected: expected)
      DispatchQueue.main.async {
        guard let self else { return }
        switch opened {
        case .failure(let refusal):
          self.send("error", refusal.fields)
        case .success(let (document, sha256)):
          self.url = URL(fileURLWithPath: path)
          PaperCore.readOnlyForms(document)
          self.pdf.document = document
          self.overlays = [:]
          for page in self.inks.keys { self.showInk(page: page) }
          if let page = (self.params["initial"] as? [String: Any])?["page_1based"] as? Int,
            let target = document.page(at: page - 1)
          {
            self.pdf.go(to: target)
          }
          self.send("ready", PaperCore.describe(document, sha256: sha256))
          self.pageChanged()
        }
      }
    }
  }

  // MARK: Commands

  private func handle(_ call: FlutterMethodCall, _ result: @escaping FlutterResult) {
    let args = call.arguments as? [String: Any] ?? [:]
    switch call.method {
    case "attach":
      attached = true
      let waiting = queued
      queued = []
      for (method, arguments) in waiting { channel.invokeMethod(method, arguments: arguments) }
      result(nil)
    case "goToPage":
      let n = args["page_1based"] as? Int ?? 1
      if let page = pdf.document?.page(at: n - 1) {
        if let region = PaperCore.region(args["region"]) {
          let rect = PaperCore.pageRect(region, on: page)
          pdf.go(to: rect, on: page)
          if args["match"] as? Bool == true {
            markMatch(rect, on: page)
            clearOfTheBar(rect, on: page)
          }
        } else {
          pdf.go(to: page)
        }
      }
      result(["page_1based": currentPage])
    case "search":
      search(args, result)
    case "setActive":
      result(["active": args["active"] as? Bool ?? true, "pending_flushed": false])
    case "loadDrawing":
      result(loadDrawing(args["layers"] as? [[String: Any]] ?? []))
    default:
      // Drawing and tools: not on a Mac.
      result(FlutterMethodNotImplemented)
    }
  }

  // MARK: Ink

  /// The newest content version this Mac's PencilKit reads.
  private static var maxContentVersion: Int {
    if #available(macOS 14.0, *) { return PKContentVersion.latest.rawValue }
    return 1
  }

  /// Saved drawings (cmd_load_drawing.json), each over its page, only
  /// read: a cleared one takes its page's away. Answers what was loaded and
  /// what this Mac can't read (a newer PencilKit's, or damaged), as the
  /// iPad does.
  private func loadDrawing(_ refs: [[String: Any]]) -> [String: Any] {
    var loaded: [[String: Any]] = []
    var unreadable: [[String: Any]] = []
    for ref in refs {
      guard let page = ref["page_1based"] as? Int, page > 0 else { continue }
      var entry: [String: Any] = [
        "page_1based": page, "ink_layer_id": ref["ink_layer_id"] ?? NSNull(),
        "revision": ref["revision"] ?? NSNull(),
      ]
      if ref["clear"] as? Bool == true {
        inks[page] = nil
        showInk(page: page)
        entry["strokes"] = 0
        loaded.append(entry)
        continue
      }
      let wire = ref["drawing"] as? [String: Any] ?? [:]
      let version = wire["content_version"] as? Int ?? 1
      guard let base64 = wire["bytes_base64"] as? String, let data = Data(base64Encoded: base64),
        version <= Self.maxContentVersion,
        let drawing = try? PKDrawing(data: data)
      else {
        entry["content_version"] = version
        entry["max_content_version"] = Self.maxContentVersion
        unreadable.append(entry)
        continue
      }
      inks[page] = drawing.strokes.isEmpty ? nil : drawing
      showInk(page: page)
      entry["strokes"] = drawing.strokes.count
      loaded.append(entry)
    }
    return ["loaded": loaded, "unreadable": unreadable]
  }

  /// [page]'s ink as it is now in [inks], over the page, or none.
  private func showInk(page n: Int) {
    if let old = overlays.removeValue(forKey: n) { old.page?.removeAnnotation(old) }
    guard let drawing = inks[n], let page = pdf.document?.page(at: n - 1) else { return }
    let overlay = InkOverlay(drawing: drawing, crop: page.bounds(for: .cropBox))
    // Under the page's own annotations: PDFKit takes a click for the last
    // one added under it (PDFPage.annotation(at:)), so the ink, as big as
    // the page, would take every link's click (Abbacchio's find).
    let own = page.annotations
    for annotation in own { page.removeAnnotation(annotation) }
    page.addAnnotation(overlay)
    for annotation in own { page.addAnnotation(annotation) }
    overlays[n] = overlay
  }

  private var currentPage: Int {
    guard let doc = pdf.document, let page = pdf.currentPage else { return 1 }
    return doc.index(for: page) + 1
  }

  private func pageChanged() {
    guard let doc = pdf.document else { return }
    send("pageChanged", ["page_1based": currentPage, "page_count": doc.pageCount])
  }

  /// The find's match gone to, selected as Preview does, so it stands
  /// apart from the others' yellow.
  private var matchShown = false

  private func markMatch(_ rect: CGRect, on page: PDFPage) {
    // The one nearest [rect]: a selection's own bounds can take in a whole
    // column of lines, so the first that meets it may be another.
    func distance(_ selection: PDFSelection) -> CGFloat {
      guard selection.pages.contains(page) else { return .infinity }
      let r = PaperCore.tightBounds(of: selection, on: page)
      return hypot(r.midX - rect.midX, r.midY - rect.midY)
    }
    let match = pdf.highlightedSelections?.min { distance($0) < distance($1) }
      .flatMap { distance($0).isFinite ? $0 : nil }
    matchShown = match != nil
    pdf.setCurrentSelection(match, animate: false)
  }

  /// Brings a find's match [rect] (page space) down into the view's upper
  /// part, clear of the find bar floating over the view's top (Abbacchio's
  /// row 560s: PDFKit's go(to:) stops with it at the very top, under the
  /// bar). A match already well in sight stays where it is.
  private func clearOfTheBar(_ rect: CGRect, on page: PDFPage) {
    guard let document = pdf.documentView, let scroll = document.enclosingScrollView else { return }
    let clip = scroll.contentView
    let shown = document.convert(pdf.convert(rect, from: page), from: pdf)
    var visible = clip.documentVisibleRect
    // Distances from the view's top, whichever way the document runs.
    let flipped = document.isFlipped
    let top = flipped ? shown.minY - visible.minY : visible.maxY - shown.maxY
    let bottom = flipped ? shown.maxY - visible.minY : visible.maxY - shown.minY
    let bar: CGFloat = 72
    guard top < bar || bottom > visible.height * 0.85 else { return }
    let mid = flipped ? shown.midY : shown.maxY - shown.height / 2
    let want = bar + (visible.height - bar) * 0.25
    visible.origin.y = flipped ? mid - want : mid + want - visible.height
    let low = document.bounds.minY, high = max(low, document.bounds.maxY - visible.height)
    visible.origin.y = min(max(visible.origin.y, low), high)
    clip.scroll(to: visible.origin)
    scroll.reflectScrolledClipView(clip)
  }

  /// Finds the text in a copy of the document of its own, off the main
  /// thread, then highlights the matches; an older search's answer says
  /// it was overtaken.
  private func search(_ args: [String: Any], _ result: @escaping FlutterResult) {
    searches += 1
    let generation = searches
    let query = (args["query"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
    let limit = max(1, min(args["max_results"] as? Int ?? 200, 1000))
    if matchShown {
      matchShown = false
      pdf.clearSelection()
    }
    guard !query.isEmpty, let url else {
      pdf.highlightedSelections = nil
      return result(["query": query, "matches": [], "truncated": false])
    }
    searching.async { [weak self] in
      let found = PDFDocument(url: url).map { PaperCore.search($0, query: query, limit: limit) }
      DispatchQueue.main.async {
        guard let self else { return }
        guard generation == self.searches else {
          return result(["query": query, "matches": [], "truncated": false, "superseded": true])
        }
        let matches = found?.matches ?? []
        self.pdf.highlightedSelections = matches.compactMap { m in
          guard let page = self.pdf.document?.page(at: m.page),
            let selection = page.selection(for: m.range)
          else { return nil }
          selection.color = NSColor.systemYellow.withAlphaComponent(0.5)
          return selection
        }
        result(PaperCore.searchJSON(query: query, matches: matches, truncated: found?.truncated ?? false))
      }
    }
  }

  // MARK: PDFViewDelegate

  /// A link out of the PDF never opens by itself: Dart offers it. Links
  /// within it go to their page, as PDFKit does.
  func pdfViewWillClick(onLink sender: PDFView, with url: URL) {
    send("linkTapped", ["url": url.absoluteString])
  }

  /// A link into another PDF file: not followed.
  func pdfViewOpenPDF(_ sender: PDFView, forRemoteGoToAction action: PDFActionRemoteGoTo) {}
}

/// A page's saved ink, drawn over the page in its own space: the drawing's
/// is the page's CropBox from its top left, y down (the iPad's
/// "page_overlay", drawing_to_document [1, 0, 0, -1, x, y + height]).
/// PDFKit turns it with the page. Only shown: never printed or saved, and
/// under the page's links, so they still take their clicks.
final class InkOverlay: PDFAnnotation {
  private let drawing: PKDrawing
  private let crop: CGRect
  /// The drawing as an image, at the pixels per point it was last drawn.
  private var rendered: (scale: CGFloat, image: CGImage)?

  init(drawing: PKDrawing, crop: CGRect) {
    self.drawing = drawing
    self.crop = crop
    super.init(bounds: crop, forType: .stamp, withProperties: nil)
    shouldPrint = false
    isReadOnly = true
  }

  required init?(coder: NSCoder) { nil }

  override func draw(with box: PDFDisplayBox, in context: CGContext) {
    // As sharp as the page is shown, in steps, so zooming doesn't redraw
    // it at every frame.
    let ctm = context.ctm
    let shown = max(hypot(ctm.a, ctm.b), hypot(ctm.c, ctm.d))
    // At most 4096 pixels along its long edge.
    let most = 4096 / max(crop.width, crop.height, 1)
    let scale = min(most, 8, max(1, (shown * 2).rounded(.up) / 2))
    if rendered?.scale != scale {
      var image: NSImage?
      // Ink as on paper: PencilKit lightens dark ink in dark mode, and the
      // page is white whatever the Mac's appearance.
      NSAppearance(named: .aqua)?.performAsCurrentDrawingAppearance {
        image = drawing.image(from: CGRect(origin: .zero, size: crop.size), scale: scale)
      }
      guard let cg = image?.cgImage(forProposedRect: nil, context: nil, hints: nil) else { return }
      rendered = (scale, cg)
    }
    guard let image = rendered?.image else { return }
    context.saveGState()
    // PDFKit gives an annotation the page unturned: turn it as the page.
    if let page { context.concatenate(page.transform(for: box)) }
    context.draw(image, in: crop)
    context.restoreGState()
  }
}
