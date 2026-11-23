// A PDF with a PencilKit canvas over each page PDFKit shows, all in one
// native hierarchy, so ink scrolls and zooms with its page in the same
// frame (ipad.md §2). PDFKit makes overlays only for pages near the screen
// and recycles them: drawings live in the surface's layers, never only in
// a view (spike).
//
// A page overlay's drawing is in points from the page's CropBox top left,
// y down, unrotated: drawing_to_document is [1, 0, 0, -1, cx, cy + ch]
// whatever the page's /Rotate (confirmed in docs/ipad-ink-spike.md).

import CryptoKit
import Flutter
import PDFKit
import PencilKit
import UIKit

@available(iOS 16.0, *)
final class PaperView: InkSurface, PDFPageOverlayViewProvider, PDFViewDelegate {
  let pdf = PDFView()
  private var canvases: [Int: InkCanvas] = [:]
  private var notes: [NSObjectProtocol] = []
  /// Scrolling, for the page shown (PDFKit's page-changed note says only
  /// when its own current page changes).
  private var scrolling: NSKeyValueObservation?
  private weak var scrollView: UIScrollView?
  private var pageCheck = false
  private var lastPage: Int?
  /// The file, for searching a copy of its own off the main thread.
  private var url: URL?
  private let searching = DispatchQueue(label: "claktory.ink.search", qos: .userInitiated)
  /// Searches started; an answer for an older one is stale.
  private var searches = 0

  /// ipad.md §4: past this a PDF is refused rather than half shown.
  static let maxPages = 2000
  /// Outline entries in `ready`, at most.
  static let maxOutline = 500

  override init(frame: CGRect, params: InkParams, channel: InkEvents) {
    super.init(frame: frame, params: params, channel: channel)
    pdf.frame = root.bounds
    pdf.autoresizingMask = [.flexibleWidth, .flexibleHeight]
    pdf.displayMode = .singlePageContinuous
    pdf.displayDirection = .vertical
    pdf.backgroundColor = UIColor(white: 0.93, alpha: 1)
    pdf.pageOverlayViewProvider = self
    // Markup mode gives the page's touches to PencilKit, turning off
    // PDFKit's selecting and double-tap: only while drawing is on.
    pdf.isInMarkupMode = !drawingOff
    pdf.delegate = self
    pdf.autoScales = true
    root.addSubview(pdf)
    root.onTraits = { [weak self] in self?.interrupt("rotate") }
    let center = NotificationCenter.default
    notes.append(
      center.addObserver(forName: .PDFViewPageChanged, object: pdf, queue: .main) {
        [weak self] _ in self?.pageChanged()
      })
    notes.append(
      center.addObserver(forName: .PDFViewSelectionChanged, object: pdf, queue: .main) {
        [weak self] _ in self?.selectionChanged()
      })
    open()
  }

  deinit {
    notes.forEach(NotificationCenter.default.removeObserver)
  }

  /// Opens the cached copy once its SHA-256 is the version the backend
  /// named; never a half-old, half-new file (contract §2).
  private func open() {
    guard let path = params.document["file"] as? String else {
      return error(["code": "document_unreadable", "recoverable": false])
    }
    let expected = params.document["sha256"] as? String ?? params.sourceSha256
    let url = URL(fileURLWithPath: path)
    DispatchQueue.global(qos: .userInitiated).async { [weak self] in
      let actual = InkHash.sha256(file: url)
      DispatchQueue.main.async {
        guard let self else { return }
        guard let actual else {
          return self.error(["code": "document_unreadable", "recoverable": false])
        }
        if let expected, actual != expected {
          return self.error([
            "code": "document_hash_mismatch", "recoverable": true,
            "expected_sha256": expected, "actual_sha256": actual,
          ])
        }
        self.url = url
        self.show(document: PDFDocument(url: url), sha256: actual)
      }
    }
  }

  private func show(document: PDFDocument?, sha256: String) {
    guard let document else {
      return error(["code": "document_unreadable", "recoverable": false])
    }
    // Never unlock: a password-protected PDF is refused, not bypassed.
    if document.isLocked {
      return error(["code": "document_locked", "recoverable": false])
    }
    if document.pageCount > Self.maxPages {
      return error([
        "code": "document_too_large", "recoverable": false, "page_count": document.pageCount,
        "max_pages": Self.maxPages,
      ])
    }
    pdf.document = document
    if let page = params.initialPage, let target = document.page(at: page - 1) {
      pdf.go(to: target)
    }
    send("ready", Self.describe(document, sha256: sha256).merging(["handwriting": handwritingJSON]) { $1 })
    pageChanged()
    if let scroll = pdf.subviews.compactMap({ $0 as? UIScrollView }).first {
      scrollView = scroll
      scrolling = scroll.observe(\.contentOffset, options: []) { [weak self] _, _ in
        self?.scrolled()
      }
    }
  }

  /// Once per turn of the main loop, however often it scrolls; after
  /// whatever moved it (a reload's answer goes first).
  private func scrolled() {
    // The person moving the page: the header follows it again, not the
    // find's match.
    if matchPage != nil, let scroll = scrollView,
      scroll.isTracking || scroll.isDragging || scroll.isDecelerating || scroll.isZooming
    {
      matchPage = nil
    }
    guard !pageCheck else { return }
    pageCheck = true
    DispatchQueue.main.async { [weak self] in
      guard let self else { return }
      self.pageCheck = false
      if self.pdf.document != nil, self.currentPage != self.lastPage { self.pageChanged() }
    }
  }

  /// What `ready` (and a reload's answer) says of a document.
  static func describe(_ document: PDFDocument, sha256: String) -> [String: Any] {
    var pages: [[String: Any]] = []
    for index in 0..<document.pageCount {
      guard let page = document.page(at: index) else { continue }
      pages.append([
        "page_1based": index + 1,
        "crop_box": box(page.bounds(for: .cropBox)),
        "media_box": box(page.bounds(for: .mediaBox)),
        "rotation": ((page.rotation % 360) + 360) % 360,
      ])
    }
    var out: [String: Any] = [
      "surface": "pdf", "source_sha256": sha256, "sha256_verified": true,
      "page_count": document.pageCount, "pages": pages, "drawing_space": "page_overlay",
    ]
    let entries = Self.outline(document)
    if !entries.isEmpty { out["outline"] = entries }
    return out
  }

  /// The PDF's own outline, depth first, for a contents list.
  static func outline(_ document: PDFDocument) -> [[String: Any]] {
    var out: [[String: Any]] = []
    func walk(_ node: PDFOutline, depth: Int) {
      for i in 0..<node.numberOfChildren {
        guard out.count < maxOutline, let child = node.child(at: i) else { return }
        // Read back from a file, an entry's page is usually its go-to action's.
        let destination = child.destination ?? (child.action as? PDFActionGoTo)?.destination
        if let page = destination?.page {
          out.append([
            "title": (child.label ?? "").trimmingCharacters(in: .whitespacesAndNewlines),
            "page_1based": document.index(for: page) + 1, "depth": depth,
          ])
        }
        walk(child, depth: depth + 1)
      }
    }
    if let root = document.outlineRoot { walk(root, depth: 0) }
    return out
  }

  static func box(_ r: CGRect) -> [Double] {
    [r.minX, r.minY, r.width, r.height].map { (Double($0) * 1000).rounded() / 1000 }
  }

  // MARK: Surface

  override var liveCanvases: [InkCanvas] { Array(canvases.values) }

  /// Drawing off, a page's overlay lets every touch through to PDFKit:
  /// scrolling, links, text selection.
  override func applyDrawing(_ canvas: InkCanvas) {
    super.applyDrawing(canvas)
    canvas.isUserInteractionEnabled = !drawingOff
  }

  override func show(_ drawing: PKDrawing, page: Int) {
    canvases[page]?.drawing = drawing
  }

  override func drawingPolicyChanged() {
    pdf.isInMarkupMode = !drawingOff
  }

  override func zoom(at point: CGPoint) -> CGFloat { pdf.scaleFactor }

  override func drawingSize(page: Int) -> CGSize {
    pdf.document?.page(at: page - 1)?.bounds(for: .cropBox).size ?? .zero
  }

  override func setActive(_ active: Bool) {
    pdf.isUserInteractionEnabled = active
  }

  /// A page's overlay: crop points from the CropBox's top left, y down,
  /// whatever the page's /Rotate (layer_pdf_*.json).
  override func layerGeometry(page: Int) -> LayerGeometry? {
    guard page > 0, let p = pdf.document?.page(at: page - 1) else { return nil }
    let crop = p.bounds(for: .cropBox)
    let rotation = ((p.rotation % 360) + 360) % 360
    let c = Self.box(crop)
    return LayerGeometry(
      coordinateSpace: [
        "kind": "pdf_page", "page_1based": page, "box": "crop", "crop_box": c,
        "media_box": Self.box(p.bounds(for: .mediaBox)), "rotation": rotation,
        "width": c[2], "height": c[3], "units": "pt",
      ],
      space: "page_overlay", drawingToDocument: [1, 0, 0, -1, c[0], c[1] + c[3]],
      size: crop.size, rotation: rotation)
  }

  override func handle(_ call: FlutterMethodCall, _ result: @escaping FlutterResult) {
    let args = call.arguments as? [String: Any] ?? [:]
    switch call.method {
    case "goToPage":
      let n = args["page_1based"] as? Int ?? 1
      if let page = pdf.document?.page(at: n - 1) {
        if let region = InkRegion.rect(args["region"]), args["match"] as? Bool == true {
          showMatch(Self.pageRect(region, on: page), on: page, n: n)
        } else if let region = InkRegion.rect(args["region"]) {
          matchPage = nil
          pdf.go(to: Self.pageRect(region, on: page), on: page)
        } else {
          matchPage = nil
          pdf.go(to: page)
        }
      }
      result(["page_1based": currentPage])
    case "search":
      search(args, result)
    case "reloadDocument":
      reload(args, result)
    default:
      super.handle(call, result)
    }
  }

  /// A region (0..1 from the unrotated CropBox's top left) in the page's
  /// own space (PDF points, y up).
  static func pageRect(_ r: CGRect, on page: PDFPage) -> CGRect {
    let crop = page.bounds(for: .cropBox)
    return CGRect(
      x: crop.minX + r.minX * crop.width, y: crop.maxY - r.maxY * crop.height,
      width: r.width * crop.width, height: r.height * crop.height)
  }

  /// The other way: a rect in page space as a region.
  static func region(_ b: CGRect, on page: PDFPage) -> CGRect {
    let crop = page.bounds(for: .cropBox)
    return CGRect(
      x: (b.minX - crop.minX) / crop.width, y: (crop.maxY - b.maxY) / crop.height,
      width: b.width / crop.width, height: b.height / crop.height
    ).intersection(CGRect(x: 0, y: 0, width: 1, height: 1))
  }

  // MARK: A new version while open

  /// A page's makings, cheaply: its boxes, rotation, content streams and
  /// the images and forms they draw. Equal makings draw an equal page.
  static func makings(_ page: PDFPage) -> String {
    var hasher = SHA256()
    let boxes = [page.bounds(for: .cropBox), page.bounds(for: .mediaBox)]
      .map { "\($0.minX),\($0.minY),\($0.width),\($0.height)" }.joined(separator: ";")
    hasher.update(data: Data("\(boxes);\(((page.rotation % 360) + 360) % 360)".utf8))
    func add(_ stream: CGPDFStreamRef) {
      var format = CGPDFDataFormat.raw
      if let data = CGPDFStreamCopyData(stream, &format) { hasher.update(data: data as Data) }
    }
    guard let dict = page.pageRef?.dictionary else { return "" }
    var contents: CGPDFObjectRef?
    if CGPDFDictionaryGetObject(dict, "Contents", &contents), let contents {
      var stream: CGPDFStreamRef?
      var array: CGPDFArrayRef?
      if CGPDFObjectGetValue(contents, .stream, &stream), let stream {
        add(stream)
      } else if CGPDFObjectGetValue(contents, .array, &array), let array {
        for i in 0..<CGPDFArrayGetCount(array) {
          var part: CGPDFStreamRef?
          if CGPDFArrayGetStream(array, i, &part), let part { add(part) }
        }
      }
    }
    var resources: CGPDFDictionaryRef?
    var objects: CGPDFDictionaryRef?
    if CGPDFDictionaryGetDictionary(dict, "Resources", &resources), let resources,
      CGPDFDictionaryGetDictionary(resources, "XObject", &objects), let objects
    {
      final class Found { var streams: [(String, CGPDFStreamRef)] = [] }
      let found = Found()
      CGPDFDictionaryApplyBlock(
        objects,
        { key, value, info in
          var stream: CGPDFStreamRef?
          if CGPDFObjectGetValue(value, .stream, &stream), let stream, let info {
            Unmanaged<Found>.fromOpaque(info).takeUnretainedValue().streams.append(
              (String(cString: key), stream))
          }
          return true
        }, Unmanaged.passUnretained(found).toOpaque())
      for (name, stream) in found.streams.sorted(by: { $0.0 < $1.0 }) {
        hasher.update(data: Data(name.utf8))
        add(stream)
      }
    }
    return hasher.finalize().map { String(format: "%02x", $0) }.joined()
  }

  /// The slower check, for a page with ink whose makings differ (a writer
  /// may rewrite a page without changing it): its text and a rendering at
  /// review size.
  static func looksTheSame(_ a: PDFPage, _ b: PDFPage) -> Bool {
    func render(_ page: PDFPage) -> String {
      let image = page.thumbnail(of: CGSize(width: 400, height: 400), for: .cropBox)
      return InkHash.sha256(image.cgImage?.dataProvider?.data as Data? ?? Data())
    }
    return a.bounds(for: .cropBox) == b.bounds(for: .cropBox) && a.rotation == b.rotation
      && a.string == b.string && render(a) == render(b)
  }

  /// The pages (1-based) that changed from one version to the next, page
  /// by page, never remapped. `inked`: pages with ink, given the closer look.
  static func changedPages(from before: PDFDocument?, to after: PDFDocument, inked: Set<Int>) -> [Int] {
    guard let before else { return [] }
    var changed: [Int] = []
    for index in 0..<min(before.pageCount, after.pageCount) {
      guard let a = before.page(at: index), let b = after.page(at: index) else { continue }
      if makings(a) == makings(b) { continue }
      if inked.contains(index + 1), looksTheSame(a, b) { continue }
      changed.append(index + 1)
    }
    return changed
  }

  /// Shows a new version of the PDF in place (contract: the reload
  /// fixtures). Pages are compared by number, never remapped; ink on a page
  /// that changed or went is no longer shown, and Dart hears it's
  /// outdated. A version that can't be shown leaves this one as it is.
  private func reload(_ args: [String: Any], _ result: @escaping FlutterResult) {
    let document = args["document"] as? [String: Any] ?? [:]
    let source = args["source"] as? [String: Any] ?? [:]
    guard let path = document["file"] as? String else {
      return result(FlutterError(code: "bad_request", message: "document.file", details: nil))
    }
    let expected = document["sha256"] as? String ?? source["sha256"] as? String
    let url = URL(fileURLWithPath: path)
    let old = self.url
    let shown = sourceSha256
    let inked = Set(layers.filter { $0.value.inkLayerId != nil || !$0.value.drawing.strokes.isEmpty }.keys)
    DispatchQueue.global(qos: .userInitiated).async { [weak self] in
      func fail(_ code: String, _ details: [String: Any]) {
        DispatchQueue.main.async {
          var details = details
          details["shown_sha256"] = shown.inkJSON
          result(FlutterError(code: code, message: nil, details: details))
        }
      }
      guard let actual = InkHash.sha256(file: url) else { return fail("document_unreadable", [:]) }
      if let expected, actual != expected {
        return fail("document_hash_mismatch", ["expected_sha256": expected, "actual_sha256": actual])
      }
      guard let fresh = PDFDocument(url: url) else { return fail("document_unreadable", [:]) }
      if fresh.isLocked { return fail("document_locked", [:]) }
      if fresh.pageCount > Self.maxPages {
        return fail("document_too_large", ["page_count": fresh.pageCount, "max_pages": Self.maxPages])
      }
      // Compared on copies of their own, off the main thread.
      let before = old.flatMap(PDFDocument.init(url:))
      let changed = Self.changedPages(from: before, to: fresh, inked: inked)
      let oldCount = before?.pageCount ?? 0
      DispatchQueue.main.async {
        guard let self else { return }
        self.show(
          reloaded: url, sha256: actual, changed: changed, oldCount: oldCount, previous: shown,
          result: result)
      }
    }
  }

  private func show(
    reloaded url: URL, sha256: String, changed: [Int], oldCount: Int, previous: String?,
    result: @escaping FlutterResult
  ) {
    guard let document = PDFDocument(url: url) else {
      return result(FlutterError(code: "document_unreadable", message: nil, details: nil))
    }
    // A stroke in progress belongs to the version going; what's pending is
    // sent for it first.
    interrupt("reload")
    flush()
    let count = document.pageCount
    let gone = Set(changed).union(count < oldCount ? Array((count + 1)...oldCount) : [])
    var kept: [[String: Any]] = []
    var outdated: [[String: Any]] = []
    for page in layers.keys.sorted() {
      guard let layer = layers[page] else { continue }
      let entry: [String: Any] = [
        "page_1based": page, "ink_layer_id": layer.inkLayerId.inkJSON,
        "revision": layer.revision.inkJSON,
      ]
      if gone.contains(page) {
        if layer.inkLayerId != nil || !layer.drawing.strokes.isEmpty { outdated.append(entry) }
        layers[page] = nil
      } else if layer.inkLayerId != nil || !layer.drawing.strokes.isEmpty {
        // Saved or not: drawn here, still shown.
        kept.append(entry)
      }
    }
    // Where the person was: the spot of the page at the view's top left.
    let spot = Self.spot(in: pdf)
    let scale = pdf.scaleFactor
    let fitted = pdf.autoScales
    searches += 1
    found = [:]
    currentMatch = nil
    matchPage = nil
    // The old version's overlays say nothing more.
    for canvas in canvases.values { canvas.delegate = nil }
    canvases.removeAll()
    undo.removeAllActions()
    self.url = url
    sourceSha256 = sha256
    pdf.document = document
    if !fitted { pdf.scaleFactor = scale }
    // Back to the same spot, if its page is still there.
    pdf.layoutDocumentView()
    if let spot, spot.index < count, let page = document.page(at: spot.index) {
      pdf.go(to: page)
      pdf.layoutDocumentView()
      if let scroll = pdf.subviews.compactMap({ $0 as? UIScrollView }).first {
        let now = pdf.convert(spot.point, from: page)
        var offset = scroll.contentOffset
        offset.x += now.x
        offset.y += now.y
        let inset = scroll.adjustedContentInset
        offset.x = min(max(offset.x, -inset.left), max(-inset.left, scroll.contentSize.width - scroll.bounds.width + inset.right))
        offset.y = min(max(offset.y, -inset.top), max(-inset.top, scroll.contentSize.height - scroll.bounds.height + inset.bottom))
        scroll.setContentOffset(offset, animated: false)
      }
    }
    var answer = Self.describe(document, sha256: sha256)
    answer["surface"] = nil
    answer["sha256_verified"] = nil
    answer["drawing_space"] = nil
    answer["reloaded"] = true
    answer["previous_sha256"] = previous.inkJSON
    answer["page_1based"] = currentPage
    answer["pages_changed"] = changed
    answer["pages_added"] = count > oldCount ? Array((oldCount + 1)...count) : []
    answer["pages_removed"] = count < oldCount ? Array((count + 1)...oldCount) : []
    answer["comparison"] = "content_then_render"
    answer["kept_layers"] = kept
    answer["outdated_layers"] = outdated
    send("undoState", undoState)
    result(answer)
    pageChanged()
  }

  /// The page under the view's top left corner and the point of it there.
  static func spot(in pdf: PDFView) -> (index: Int, point: CGPoint)? {
    guard let document = pdf.document,
      let page = pdf.page(for: .zero, nearest: true)
    else { return nil }
    return (document.index(for: page), pdf.convert(CGPoint.zero, to: page))
  }

  // MARK: Search

  private struct Match {
    let page: Int
    let region: CGRect
    let text: String
    /// Its washes, a line each, in its page overlay's space.
    let lines: [CGRect]
  }

  /// A find's matches by page (1-based), in the page overlay's space.
  private(set) var found: [Int: [CGRect]] = [:]

  /// Shows [washes] (page 1-based, overlay space) as the find's
  /// highlights, in place of the last: each page's overlay washes its own
  /// (InkCanvas.found).
  func highlight(_ washes: [(page: Int, rect: CGRect)]) {
    var byPage: [Int: [CGRect]] = [:]
    for wash in washes { byPage[wash.page, default: []].append(wash.rect) }
    found = byPage
    currentMatch = nil
    matchPage = nil
    for (n, canvas) in canvases {
      canvas.found = byPage[n] ?? []
      canvas.current = nil
    }
  }

  /// The find's current match (page 1-based, in its overlay's space).
  private(set) var currentMatch: (page: Int, rect: CGRect)?

  /// The page of the match gone to, which the header names until the
  /// person scrolls (Zucchero's row 137: raised high near a page's foot,
  /// the match left the next page filling more of the view, and the
  /// header named that one).
  private(set) var matchPage: Int?

  /// Goes to a find's match [rect] (page space) on [page]: marked apart
  /// from the others, and brought into the view's upper part, where a find
  /// bar and keyboard at its foot leave it in sight (Zucchero's row 91:
  /// PDFKit's go(to:) left a match under them, the view thinking it shown).
  /// A match already well in sight stays where it is.
  func showMatch(_ rect: CGRect, on page: PDFPage, n: Int) {
    let mark = Self.overlayRect(rect, on: page)
    currentMatch = (n, mark)
    matchPage = n
    defer { if currentPage != lastPage { pageChanged() } }
    for (index, canvas) in canvases { canvas.current = index == n ? mark : nil }
    guard let scroll = scrollView, pdf.bounds.height > 0 else {
      return pdf.go(to: rect, on: page)
    }
    // Laid out, if it's a page not drawn yet.
    if pdf.convert(rect, from: page).isEmpty { pdf.go(to: page) }
    let shown = pdf.convert(rect, from: page)
    let height = pdf.bounds.height
    let width = pdf.bounds.width
    var offset = scroll.contentOffset
    if shown.minY < height * 0.08 || shown.maxY > height * 0.45 {
      offset.y += shown.midY - height * 0.25
    }
    if shown.minX < 0 || shown.maxX > width {
      offset.x += shown.midX - width / 2
    }
    let inset = scroll.adjustedContentInset
    offset.y = min(
      max(offset.y, -inset.top),
      max(-inset.top, scroll.contentSize.height - scroll.bounds.height + inset.bottom))
    offset.x = min(
      max(offset.x, -inset.left),
      max(-inset.left, scroll.contentSize.width - scroll.bounds.width + inset.right))
    if offset != scroll.contentOffset { scroll.setContentOffset(offset, animated: false) }
  }

  /// A match's bounds on [page] (page space): the union of its
  /// characters', each as PDFKit selects it. A selection's own bounds, and
  /// its lines', can take in far more (the whole column of lines in a
  /// monospaced report, Zucchero's row 91), and PDFPage.characterBounds
  /// can be a line or a character away across (row 117). Some PDFs place
  /// a line's first few characters far too wide: those count as wide as
  /// the rest, so a match starting a line is washed from its own start.
  /// Some make every character as tall as the page: then the height is
  /// characterBounds', which keeps to the line.
  static func tightBounds(of selection: PDFSelection, on page: PDFPage) -> CGRect {
    let range = selection.range(at: 0, on: page)
    guard range.location != NSNotFound, range.length > 0, range.length <= 500 else {
      return selection.bounds(for: page)
    }
    let indices = range.location..<(range.location + range.length)
    let glyphs = indices.map {
      page.selection(for: NSRange(location: $0, length: 1))?.bounds(for: page) ?? .null
    }
    let placed = glyphs.filter { !$0.isNull && $0.width > 0 && $0.height > 0 }
    guard !placed.isEmpty else { return selection.bounds(for: page) }
    // The characters' usual width: of those as wide as another, within a
    // seventh, since a line's misplaced first few can be most of a match.
    let widths = placed.map(\.width)
    let alike = widths.filter { w in widths.filter { abs($0 - w) <= w / 7 }.count > 1 }.sorted()
    let width = alike.isEmpty ? widths.sorted()[widths.count / 2] : alike[alike.count / 2]
    let height = placed.map(\.height).sorted()[placed.count / 2]
    let sound = glyphs.indices.filter {
      let g = glyphs[$0]
      return !g.isNull && g.width >= width * 0.4 && g.width <= width * 1.75 && g.height > 0
        && g.height <= height * 2.5
    }
    guard let first = sound.first, let last = sound.last else {
      return selection.bounds(for: page)
    }
    var bounds = sound.reduce(CGRect.null) { $0.union(glyphs[$1]) }
    let before = CGFloat(first), after = CGFloat(glyphs.count - 1 - last)
    bounds.origin.x -= before * width
    bounds.size.width += (before + after) * width
    if height > width * 6 {
      let ink = indices.map { page.characterBounds(at: $0) }.filter { $0.height > 0 }
      if let low = ink.map(\.minY).min(), let high = ink.map(\.maxY).max(), high > low {
        let pad = (high - low) * 0.05
        bounds.origin.y = low - pad
        bounds.size.height = high - low + 2 * pad
      }
    }
    return bounds
  }

  /// [r] (page space) in [page]'s overlay space: from its CropBox's top
  /// left, y down.
  static func overlayRect(_ r: CGRect, on page: PDFPage) -> CGRect {
    let crop = page.bounds(for: .cropBox)
    return CGRect(x: r.minX - crop.minX, y: crop.maxY - r.maxY, width: r.width, height: r.height)
  }

  /// Finds the text in a copy of the document of its own, off the main
  /// thread, then highlights the matches on the pages shown.
  private func search(_ args: [String: Any], _ result: @escaping FlutterResult) {
    searches += 1
    let generation = searches
    let query = (args["query"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
    let limit = max(1, min(args["max_results"] as? Int ?? 200, 1000))
    guard !query.isEmpty, let url else {
      highlight([])
      return result(["query": query, "matches": [], "truncated": false])
    }
    searching.async { [weak self] in
      var matches: [Match] = []
      var truncated = false
      if let copy = PDFDocument(url: url) {
        for selection in copy.findString(query, withOptions: [.caseInsensitive, .diacriticInsensitive]) {
          guard let page = selection.pages.first else { continue }
          if matches.count == limit {
            truncated = true
            break
          }
          let line = selection.copy() as? PDFSelection ?? selection
          line.extendForLineBoundaries()
          let lines = selection.selectionsByLine().flatMap { line in
            line.pages.filter { $0 == page }.map {
              Self.overlayRect(Self.tightBounds(of: line, on: $0), on: $0)
            }
          }
          matches.append(
            Match(
              page: copy.index(for: page),
              region: Self.region(Self.tightBounds(of: selection, on: page), on: page),
              text: (line.string ?? query).trimmingCharacters(in: .whitespacesAndNewlines),
              lines: lines))
        }
      }
      DispatchQueue.main.async {
        guard let self else { return }
        guard generation == self.searches else {
          return result(["query": query, "matches": [], "truncated": false, "superseded": true])
        }
        self.highlight(matches.flatMap { m in m.lines.map { (page: m.page + 1, rect: $0) } })
        result([
          "query": query, "truncated": truncated,
          "matches": matches.map {
            ["page_1based": $0.page + 1, "region": InkRegion.json($0.region), "text": $0.text]
          },
        ])
      }
    }
  }

  /// The page shown most: PDFKit's own current page is the one across the
  /// view's middle, which in a tall, narrow window is often the second of
  /// two whole pages.
  private var currentPage: Int {
    if let page = matchPage { return page }
    if let doc = pdf.document, let scroll = scrollView, doc.pageCount > 0, Self.atEnd(scroll) {
      return doc.pageCount
    }
    guard let doc = pdf.document, let page = Self.mostShown(in: pdf) ?? pdf.currentPage else {
      return 1
    }
    return doc.index(for: page) + 1
  }

  /// Scrolled to the end of a document longer than the view (within a
  /// point): the last page, as Preview says, though the one before it may
  /// show as much.
  static func atEnd(_ scroll: UIScrollView) -> Bool {
    let inset = scroll.adjustedContentInset
    let top = -inset.top
    let end = scroll.contentSize.height - scroll.bounds.height + inset.bottom
    return end > top + 1 && scroll.contentOffset.y >= end - 1
  }

  /// The visible page with the most of it in view; of pages shown about
  /// as much (within 1%: a whole page a fraction of a point off the top is
  /// still whole), the first.
  static func mostShown(in pdf: PDFView) -> PDFPage? {
    guard let doc = pdf.document else { return nil }
    var best: (page: PDFPage, index: Int, area: CGFloat)?
    for page in pdf.visiblePages {
      let shown = pdf.convert(page.bounds(for: pdf.displayBox), from: page).intersection(pdf.bounds)
      guard !shown.isNull, !shown.isEmpty else { continue }
      let area = shown.width * shown.height
      let index = doc.index(for: page)
      if let had = best {
        let more = area > had.area * 1.01
        let asMuch = area * 1.01 >= had.area
        guard more || (asMuch && index < had.index) else { continue }
      }
      best = (page, index, area)
    }
    return best?.page
  }

  private func pageChanged() {
    guard let doc = pdf.document else { return }
    let page = currentPage
    lastPage = page
    send("pageChanged", ["page_1based": page, "page_count": doc.pageCount])
    refreshStatus()
  }

  override var statusText: (label: String, page: Int)? {
    guard let doc = pdf.document else { return nil }
    return ("Page \(currentPage) of \(doc.pageCount)", currentPage)
  }

  override func readingOrder() -> [Any] { [pdf] }

  /// A text selection, for a review item's extracted quote (§3).
  private func selectionChanged() {
    guard let doc = pdf.document, let selection = pdf.currentSelection,
      let page = selection.pages.first, let text = selection.string, !text.isEmpty
    else { return }
    send(
      "selectionChanged",
      [
        "page_1based": doc.index(for: page) + 1, "text": text,
        "region": InkRegion.json(Self.region(selection.bounds(for: page), on: page)),
      ])
  }

  /// Links out of the PDF never open by themselves (§4): Dart hears of
  /// the tap and offers to follow it. Links within it go to their page.
  func pdfViewWillClick(onLink sender: PDFView, with url: URL) {
    send("linkTapped", ["url": url.absoluteString])
  }

  // MARK: PDFPageOverlayViewProvider

  func pdfView(_ view: PDFView, overlayViewFor page: PDFPage) -> UIView? {
    guard let index = view.document?.index(for: page) else { return nil }
    let n = index + 1
    // Form fields show what they hold, and can't be filled in or submitted.
    for annotation in page.annotations where annotation.type == "Widget" {
      annotation.isReadOnly = true
    }
    if let canvas = canvases[n] { return canvas }
    let canvas = makeCanvas(page: n)
    canvas.found = found[n] ?? []
    canvas.current = currentMatch?.page == n ? currentMatch?.rect : nil
    canvases[n] = canvas
    return canvas
  }

  func pdfView(
    _ pdfView: PDFView, willEndDisplayingOverlayView overlayView: UIView, for page: PDFPage
  ) {
    // Only this surface's current canvas for the page: an overlay of the
    // version before a reload holds ink that may be outdated.
    guard let canvas = overlayView as? InkCanvas, canvases[canvas.tag] === canvas else { return }
    if var layer = layers[canvas.tag] {
      layer.drawing = canvas.drawing
      layers[canvas.tag] = layer
    }
    canvases[canvas.tag] = nil
  }

  // MARK: Export

  override func exportComposite(_ args: [String: Any], _ result: @escaping FlutterResult) {
    let n = args["page_1based"] as? Int ?? currentPage
    // No region: the whole page, for a preview.
    let region = InkRegion.rect(args["region"]) ?? CGRect(x: 0, y: 0, width: 1, height: 1)
    guard let page = pdf.document?.page(at: n - 1)
    else { return result(FlutterError(code: "bad_request", message: "page", details: nil)) }
    let drawing = layers[n]?.drawing ?? PKDrawing()
    let request = InkExport.Request(args)
    // On the main thread: PencilKit renders the ink there.
    guard var out = InkExport.pdf(page: page, region: region, drawing: drawing, request: request)
    else { return result(FlutterError(code: "export_too_large", message: nil, details: nil)) }
    out["source_sha256"] = sourceSha256.inkJSON
    result(out)
  }
}
