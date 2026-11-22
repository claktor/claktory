// A PDF with a PencilKit canvas over each page PDFKit shows, all in one
// native hierarchy, so ink scrolls and zooms with its page in the same
// frame (ipad.md §2). PDFKit makes overlays only for pages near the screen
// and recycles them: drawings live in the surface's layers, never only in
// a view (spike).
//
// A page overlay's drawing is in points from the page's CropBox top left,
// y down, unrotated: drawing_to_document is [1, 0, 0, -1, cx, cy + ch]
// whatever the page's /Rotate (confirmed in docs/ipad-ink-spike.md).

import Flutter
import PDFKit
import PencilKit
import UIKit

@available(iOS 16.0, *)
final class PaperView: InkSurface, PDFPageOverlayViewProvider, PDFViewDelegate {
  let pdf = PDFView()
  private var canvases: [Int: InkCanvas] = [:]
  private var notes: [NSObjectProtocol] = []

  override init(frame: CGRect, params: InkParams, channel: InkEvents) {
    super.init(frame: frame, params: params, channel: channel)
    pdf.frame = root.bounds
    pdf.autoresizingMask = [.flexibleWidth, .flexibleHeight]
    pdf.displayMode = .singlePageContinuous
    pdf.displayDirection = .vertical
    pdf.backgroundColor = UIColor(white: 0.93, alpha: 1)
    pdf.pageOverlayViewProvider = self
    pdf.isInMarkupMode = true
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
    pdf.document = document
    if let page = params.initialPage, let target = document.page(at: page - 1) {
      pdf.go(to: target)
    }
    var pages: [[String: Any]] = []
    for index in 0..<document.pageCount {
      guard let page = document.page(at: index) else { continue }
      pages.append([
        "page_1based": index + 1,
        "crop_box": Self.box(page.bounds(for: .cropBox)),
        "media_box": Self.box(page.bounds(for: .mediaBox)),
        "rotation": ((page.rotation % 360) + 360) % 360,
      ])
    }
    send(
      "ready",
      [
        "surface": "pdf", "source_sha256": sha256, "sha256_verified": true,
        "page_count": document.pageCount, "pages": pages, "drawing_space": "page_overlay",
      ])
    pageChanged()
  }

  static func box(_ r: CGRect) -> [Double] {
    [r.minX, r.minY, r.width, r.height].map { (Double($0) * 1000).rounded() / 1000 }
  }

  // MARK: Surface

  override var liveCanvases: [InkCanvas] { Array(canvases.values) }

  override func show(_ drawing: PKDrawing, page: Int) {
    canvases[page]?.drawing = drawing
  }

  override func drawingSize(page: Int) -> CGSize {
    pdf.document?.page(at: page - 1)?.bounds(for: .cropBox).size ?? .zero
  }

  override func setActive(_ active: Bool) {
    pdf.isUserInteractionEnabled = active
  }

  override func handle(_ call: FlutterMethodCall, _ result: @escaping FlutterResult) {
    guard call.method == "goToPage" else { return super.handle(call, result) }
    let args = call.arguments as? [String: Any] ?? [:]
    let n = args["page_1based"] as? Int ?? 1
    if let page = pdf.document?.page(at: n - 1) { pdf.go(to: page) }
    result(["page_1based": currentPage])
  }

  private var currentPage: Int {
    guard let doc = pdf.document, let page = pdf.currentPage else { return 1 }
    return doc.index(for: page) + 1
  }

  private func pageChanged() {
    guard let doc = pdf.document else { return }
    send("pageChanged", ["page_1based": currentPage, "page_count": doc.pageCount])
  }

  /// A text selection, for a review item's extracted quote (§3).
  private func selectionChanged() {
    guard let doc = pdf.document, let selection = pdf.currentSelection,
      let page = selection.pages.first, let text = selection.string, !text.isEmpty
    else { return }
    let crop = page.bounds(for: .cropBox)
    let b = selection.bounds(for: page)
    let region = CGRect(
      x: (b.minX - crop.minX) / crop.width, y: (crop.maxY - b.maxY) / crop.height,
      width: b.width / crop.width, height: b.height / crop.height)
    send(
      "selectionChanged",
      [
        "page_1based": doc.index(for: page) + 1, "text": text,
        "region": InkRegion.json(region.intersection(CGRect(x: 0, y: 0, width: 1, height: 1))),
      ])
  }

  /// Links never open by themselves (§4): following one is the person's
  /// deliberate choice, offered by the app.
  func pdfViewWillClick(onLink sender: PDFView, with url: URL) {}

  // MARK: PDFPageOverlayViewProvider

  func pdfView(_ view: PDFView, overlayViewFor page: PDFPage) -> UIView? {
    guard let index = view.document?.index(for: page) else { return nil }
    let n = index + 1
    if let canvas = canvases[n] { return canvas }
    let canvas = makeCanvas(page: n)
    canvases[n] = canvas
    return canvas
  }

  func pdfView(
    _ pdfView: PDFView, willEndDisplayingOverlayView overlayView: UIView, for page: PDFPage
  ) {
    guard let canvas = overlayView as? InkCanvas else { return }
    if var layer = layers[canvas.tag] {
      layer.drawing = canvas.drawing
      layers[canvas.tag] = layer
    }
    canvases[canvas.tag] = nil
  }

  // MARK: Export

  override func exportComposite(_ args: [String: Any], _ result: @escaping FlutterResult) {
    let n = args["page_1based"] as? Int ?? currentPage
    guard let page = pdf.document?.page(at: n - 1), let region = InkRegion.rect(args["region"])
    else { return result(FlutterError(code: "bad_request", message: "page or region", details: nil)) }
    let drawing = layers[n]?.drawing ?? PKDrawing()
    let request = InkExport.Request(args)
    // On the main thread: PencilKit renders the ink there.
    guard var out = InkExport.pdf(page: page, region: region, drawing: drawing, request: request)
    else { return result(FlutterError(code: "export_too_large", message: nil, details: nil)) }
    out["source_sha256"] = params.sourceSha256.inkJSON
    result(out)
  }
}
