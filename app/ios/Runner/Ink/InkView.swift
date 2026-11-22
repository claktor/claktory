// One canvas over an image, a frozen excerpt or a blank sketch (ipad.md
// §2–§3). The drawing is in the document's own units, whatever the zoom:
// oriented image pixels, the excerpt rendition's points, or the sketch's
// points (its PNG is twice that). Fingers pan and zoom unless the person
// chose Draw with finger.

import Flutter
import PencilKit
import UIKit

final class InkView: InkSurface {
  private var canvas: InkCanvas!
  /// What's under the ink: the image, the excerpt's text, or paper.
  private let underlay = UIView()
  private var image: UIImage?
  /// The document's size in drawing units.
  private(set) var size = CGSize(width: 1024, height: 768)
  /// Document units per drawing unit (drawing_to_document's scale).
  private(set) var documentScale: CGFloat = 1
  private var zoomedByPerson = false

  override init(frame: CGRect, params: InkParams, channel: InkEvents) {
    super.init(frame: frame, params: params, channel: channel)
    canvas = makeCanvas(page: 0)
    canvas.frame = root.bounds
    canvas.autoresizingMask = [.flexibleWidth, .flexibleHeight]
    canvas.bouncesZoom = true
    canvas.contentInsetAdjustmentBehavior = .never
    underlay.backgroundColor = params.paper
    canvas.insertSubview(underlay, at: 0)
    root.addSubview(canvas)
    root.onTraits = { [weak self] in self?.interrupt("rotate") }
    root.onLayout = { [weak self] _ in self?.fit() }
    open()
  }

  private func open() {
    switch params.surface {
    case "image":
      guard let path = params.document["file"] as? String else {
        return error(["code": "document_unreadable", "recoverable": false])
      }
      let expected = params.document["sha256"] as? String ?? params.sourceSha256
      DispatchQueue.global(qos: .userInitiated).async { [weak self] in
        let data = try? Data(contentsOf: URL(fileURLWithPath: path))
        let actual = data.map(InkHash.sha256)
        // UIImage applies the EXIF orientation: its size is the oriented
        // image's, the space the region and drawing are in.
        let image = data.flatMap(UIImage.init(data:))
        DispatchQueue.main.async {
          guard let self else { return }
          guard let actual, let image else {
            return self.error(["code": "document_unreadable", "recoverable": false])
          }
          if let expected, actual != expected {
            return self.error([
              "code": "document_hash_mismatch", "recoverable": true,
              "expected_sha256": expected, "actual_sha256": actual,
            ])
          }
          self.show(image: image, sha256: actual)
        }
      }
    case "excerpt":
      let excerpt = params.document["excerpt"] as? [String: Any] ?? [:]
      let text = excerpt["text"] as? String ?? ""
      let rendition = InkExcerpt(excerpt["rendition"] as? [String: Any] ?? [:])
      let label = InkExcerptView(text: text, rendition: rendition)
      size = label.size
      underlay.addSubview(label)
      label.frame = underlay.bounds
      label.autoresizingMask = [.flexibleWidth, .flexibleHeight]
      ready(sha256: params.sourceSha256, space: "excerpt_rendition")
    default:
      let blank = params.document["blank"] as? [String: Any] ?? [:]
      size = CGSize(
        width: (blank["width"] as? NSNumber)?.doubleValue ?? 1024,
        height: (blank["height"] as? NSNumber)?.doubleValue ?? 768)
      // A sketch is saved as a PNG at twice its points (layer_blank_sketch).
      documentScale = 2
      ready(sha256: params.sourceSha256, space: "blank_canvas")
    }
  }

  private func show(image: UIImage, sha256: String) {
    self.image = image
    size = CGSize(width: image.size.width * image.scale, height: image.size.height * image.scale)
    let view = UIImageView(image: image)
    view.frame = underlay.bounds
    view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
    view.contentMode = .scaleToFill
    underlay.addSubview(view)
    ready(sha256: sha256, space: "oriented_image")
  }

  private func ready(sha256: String?, space: String) {
    fit()
    send(
      "ready",
      [
        "surface": params.surface, "source_sha256": sha256.inkJSON,
        "sha256_verified": params.surface == "image",
        "page_count": 1,
        "pages": [
          [
            "page_1based": NSNull(), "width": Double(size.width), "height": Double(size.height),
          ]
        ],
        "drawing_space": space,
      ])
  }

  // MARK: Fitting and zoom

  /// Whole document in view, until the person zooms; the content size
  /// follows the zoom, as PencilKit expects.
  private func fit() {
    let bounds = root.bounds.size
    guard bounds.width > 0, bounds.height > 0, size.width > 0, size.height > 0 else { return }
    let fitScale = min(bounds.width / size.width, bounds.height / size.height)
    canvas.minimumZoomScale = fitScale
    canvas.maximumZoomScale = max(fitScale * 8, 4)
    if !zoomedByPerson || canvas.zoomScale < fitScale {
      canvas.zoomScale = fitScale
    }
    layoutContent()
  }

  private func layoutContent() {
    let scaled = CGSize(width: size.width * canvas.zoomScale, height: size.height * canvas.zoomScale)
    canvas.contentSize = scaled
    underlay.frame = CGRect(origin: .zero, size: scaled)
    // Centred while smaller than the view.
    let bounds = canvas.bounds.size
    canvas.contentInset = UIEdgeInsets(
      top: max(0, (bounds.height - scaled.height) / 2), left: max(0, (bounds.width - scaled.width) / 2),
      bottom: 0, right: 0)
  }

  func scrollViewDidZoom(_ scrollView: UIScrollView) {
    layoutContent()
  }

  func scrollViewWillBeginZooming(_ scrollView: UIScrollView, with view: UIView?) {
    zoomedByPerson = true
  }

  // MARK: Surface

  override var liveCanvases: [InkCanvas] { canvas.map { [$0] } ?? [] }

  override func show(_ drawing: PKDrawing, page: Int) {
    canvas.drawing = drawing
  }

  override func drawingSize(page: Int) -> CGSize { size }

  override func setActive(_ active: Bool) {
    canvas.isUserInteractionEnabled = active
  }

  override func exportComposite(_ args: [String: Any], _ result: @escaping FlutterResult) {
    guard let region = InkRegion.rect(args["region"]) else {
      return result(FlutterError(code: "bad_request", message: "region", details: nil))
    }
    let request = InkExport.Request(args)
    let drawing = layers[0]?.drawing ?? PKDrawing()
    let source = params.sourceSha256
    let under = InkExport.Underlay(
      size: size, image: image, paper: params.paper, view: image == nil ? underlay : nil,
      documentScale: documentScale)
    // The underlay view (excerpt text, paper) renders on the main thread.
    guard var out = InkExport.flat(underlay: under, region: region, drawing: drawing, request: request)
    else { return result(FlutterError(code: "export_too_large", message: nil, details: nil)) }
    out["source_sha256"] = source.inkJSON
    result(out)
  }
}

/// How an excerpt is set: the frozen rendition its ink was drawn on.
struct InkExcerpt {
  let width: CGFloat
  let font: UIFont
  let lineHeight: CGFloat
  let padding: CGFloat

  init(_ raw: [String: Any]) {
    width = CGFloat((raw["width"] as? NSNumber)?.doubleValue ?? 640)
    let pointSize = CGFloat((raw["font_size"] as? NSNumber)?.doubleValue ?? 17)
    let family = raw["font"] as? String ?? "Geist"
    font = InkFonts.font(family, size: pointSize)
    lineHeight = CGFloat((raw["line_height"] as? NSNumber)?.doubleValue ?? 1.5)
    padding = CGFloat((raw["padding"] as? NSNumber)?.doubleValue ?? 24)
  }
}

/// The excerpt's text, set once at its rendition's width.
final class InkExcerptView: UIView {
  let text: NSAttributedString
  let size: CGSize
  private let padding: CGFloat

  init(text: String, rendition: InkExcerpt) {
    let style = NSMutableParagraphStyle()
    style.lineHeightMultiple = rendition.lineHeight
    self.text = NSAttributedString(
      string: text,
      attributes: [
        .font: rendition.font, .paragraphStyle: style, .foregroundColor: UIColor.black,
      ])
    padding = rendition.padding
    let inner = rendition.width - 2 * padding
    let height = self.text.boundingRect(
      with: CGSize(width: inner, height: .greatestFiniteMagnitude),
      options: [.usesLineFragmentOrigin, .usesFontLeading], context: nil
    ).height
    size = CGSize(width: rendition.width, height: ceil(height + 2 * padding))
    super.init(frame: CGRect(origin: .zero, size: size))
    backgroundColor = .clear
    contentMode = .redraw
  }

  required init?(coder: NSCoder) { nil }

  override func draw(_ rect: CGRect) {
    // Drawn at the rendition's size and scaled with the view, so the
    // words stay where the ink was put.
    guard let cg = UIGraphicsGetCurrentContext() else { return }
    cg.scaleBy(x: bounds.width / size.width, y: bounds.height / size.height)
    text.draw(
      with: CGRect(x: padding, y: padding, width: size.width - 2 * padding, height: size.height),
      options: [.usesLineFragmentOrigin, .usesFontLeading], context: nil)
  }
}

/// The app's own fonts, from its Flutter assets, for natively set text.
enum InkFonts {
  private static var registered = false

  static func font(_ family: String, size: CGFloat) -> UIFont {
    registerOnce()
    return UIFont(name: family == "Geist" ? "Geist-Regular" : family, size: size)
      ?? UIFont(name: family, size: size) ?? .systemFont(ofSize: size)
  }

  private static func registerOnce() {
    guard !registered else { return }
    registered = true
    let key = FlutterDartProject.lookupKey(forAsset: "fonts/Geist-Regular.ttf")
    if let path = Bundle.main.path(forResource: key, ofType: nil) {
      CTFontManagerRegisterFontsForURL(URL(fileURLWithPath: path) as CFURL, .process, nil)
    }
  }
}
