// A review item's context image (ipad.md §3): the chosen region of the
// document with its ink, as a fresh PNG (no metadata), within the long
// edge and byte size read_image passes through unchanged, and the affine
// transform from its pixels back to the document. The rendering never
// includes anything but the document and its ink: no other panes, no
// dialogs, no notifications.

import PDFKit
import PencilKit
import UIKit

enum InkExport {
  struct Request {
    let maxLongEdge: CGFloat
    let maxBytes: Int
    let includeInk: Bool

    init(_ args: [String: Any]) {
      maxLongEdge = CGFloat((args["max_long_edge"] as? NSNumber)?.doubleValue ?? 2048)
      maxBytes = (args["max_bytes"] as? NSNumber)?.intValue ?? 3 * 1024 * 1024
      includeInk = args["include_ink"] as? Bool ?? true
    }
  }

  /// What lies under the ink of a single-canvas surface.
  struct Underlay {
    let size: CGSize
    let image: UIImage?
    let paper: UIColor
    let view: UIView?
    /// Document units per drawing unit.
    let documentScale: CGFloat
  }

  // MARK: PDF

  /// [region] is in 0..1 of the page's unrotated CropBox, from its top left.
  /// The image is turned as the page is shown (/Rotate).
  static func pdf(page: PDFPage, region: CGRect, drawing: PKDrawing, request: Request)
    -> [String: Any]?
  {
    let crop = page.bounds(for: .cropBox)
    let rotation = ((page.rotation % 360) + 360) % 360
    let r = CGRect(
      x: region.minX * crop.width, y: region.minY * crop.height,
      width: region.width * crop.width, height: region.height * crop.height)
    guard r.width > 0, r.height > 0, let pageRef = page.pageRef else { return nil }
    return fit(units: r.size, maxScale: 6, request: request) { scale in
      let flat = render(size: r.size, scale: scale) { cg in
        cg.translateBy(x: -r.minX, y: -r.minY)
        cg.saveGState()
        // Top-left, y-down crop points → PDF user space.
        cg.translateBy(x: 0, y: crop.height)
        cg.scaleBy(x: 1, y: -1)
        cg.translateBy(x: -crop.minX, y: -crop.minY)
        cg.clip(to: crop)
        cg.drawPDFPage(pageRef)
        cg.restoreGState()
        if request.includeInk { ink(drawing, rect: r, scale: scale).draw(in: r) }
      }
      let image = rotated(flat, degrees: rotation)
      let t = transform { u, v in
        // Composite pixels → shown points → unrotated region points.
        let X = u / scale, Y = v / scale
        let (x, y): (CGFloat, CGFloat)
        switch rotation {
        case 90: (x, y) = (Y, r.height - X)
        case 180: (x, y) = (r.width - X, r.height - Y)
        case 270: (x, y) = (r.width - Y, X)
        default: (x, y) = (X, Y)
        }
        return CGPoint(x: crop.minX + r.minX + x, y: crop.minY + crop.height - (r.minY + y))
      }
      return (image, t)
    }.flatMap { finish($0, region: region, drawing: drawing) }
  }

  // MARK: Image, excerpt, sketch

  /// [region] is in 0..1 of the document; the transform maps to document
  /// units (oriented image pixels, rendition points, sketch PNG pixels).
  static func flat(underlay: Underlay, region: CGRect, drawing: PKDrawing, request: Request)
    -> [String: Any]?
  {
    let size = underlay.size
    let r = CGRect(
      x: region.minX * size.width, y: region.minY * size.height,
      width: region.width * size.width, height: region.height * size.height)
    guard r.width > 0, r.height > 0 else { return nil }
    let maxScale: CGFloat = underlay.image == nil ? 3 : 2
    return fit(units: r.size, maxScale: maxScale, request: request) { scale in
      let image = render(size: r.size, scale: scale) { cg in
        cg.translateBy(x: -r.minX, y: -r.minY)
        underlay.paper.setFill()
        cg.fill(CGRect(origin: .zero, size: size))
        if let picture = underlay.image {
          picture.draw(in: CGRect(origin: .zero, size: size))
        } else if let view = underlay.view {
          for sub in view.subviews {
            sub.drawHierarchy(in: CGRect(origin: .zero, size: size), afterScreenUpdates: false)
          }
        }
        if request.includeInk { ink(drawing, rect: r, scale: scale).draw(in: r) }
      }
      let k = underlay.documentScale
      let t = transform { u, v in
        CGPoint(x: (r.minX + u / scale) * k, y: (r.minY + v / scale) * k)
      }
      return (image, t)
    }.flatMap { finish($0, region: region, drawing: drawing) }
  }

  // MARK: A layer's preview

  /// The ink alone, on transparent, turned as the document is shown: at 2
  /// pixels per drawing unit, within [maxLongEdge] and [maxBytes]
  /// (layer_*.json's `ink_preview`), for other devices to lay over their
  /// own rendering of the page.
  static func preview(
    _ drawing: PKDrawing, size: CGSize, rotation: Int, maxLongEdge: CGFloat, maxBytes: Int
  ) -> (png: Data, width: Int, height: Int)? {
    guard size.width > 0, size.height > 0 else { return nil }
    var scale = min(2, maxLongEdge / max(size.width, size.height))
    for _ in 0..<12 {
      let format = UIGraphicsImageRendererFormat()
      format.scale = 1
      format.opaque = false
      let pixels = CGSize(width: (size.width * scale).rounded(), height: (size.height * scale).rounded())
      let rect = CGRect(origin: .zero, size: size)
      let flat = UIGraphicsImageRenderer(size: pixels, format: format).image { context in
        context.cgContext.scaleBy(x: scale, y: scale)
        ink(drawing, rect: rect, scale: scale).draw(in: rect)
      }
      let image = turned(flat, degrees: rotation)
      if let png = image.pngData(), png.count <= maxBytes, let cg = image.cgImage {
        return (png, cg.width, cg.height)
      }
      scale *= 0.85
    }
    return nil
  }

  /// A temporary file of Dart's to upload and delete (ink-export, 0600,
  /// protected while the iPad is locked).
  static func write(_ data: Data, ext: String) -> URL? {
    let dir = FileManager.default.temporaryDirectory.appendingPathComponent("ink-export")
    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    let url = dir.appendingPathComponent("\(UUID().uuidString).\(ext)")
    guard (try? data.write(to: url, options: [.atomic, .completeFileProtection])) != nil else {
      return nil
    }
    try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    return url
  }

  /// [rotated], keeping what's transparent.
  private static func turned(_ image: UIImage, degrees: Int) -> UIImage {
    guard degrees != 0, let cg = image.cgImage else { return image }
    let w = CGFloat(cg.width), h = CGFloat(cg.height)
    let size = degrees == 180 ? CGSize(width: w, height: h) : CGSize(width: h, height: w)
    let format = UIGraphicsImageRendererFormat()
    format.scale = 1
    format.opaque = false
    return UIGraphicsImageRenderer(size: size, format: format).image { context in
      let c = context.cgContext
      c.translateBy(x: size.width / 2, y: size.height / 2)
      c.rotate(by: CGFloat(degrees) * .pi / 180)
      image.draw(in: CGRect(x: -w / 2, y: -h / 2, width: w, height: h))
    }
  }

  // MARK: Parts

  /// Renders at the largest scale within the long edge (and [maxScale]
  /// pixels per unit), stepping down until the PNG fits the byte limit.
  private static func fit(
    units: CGSize, maxScale: CGFloat, request: Request,
    make: (CGFloat) -> (UIImage, [Double])
  ) -> (Data, UIImage, [Double])? {
    var scale = min(request.maxLongEdge / max(units.width, units.height), maxScale)
    for _ in 0..<12 {
      let (image, t) = make(scale)
      if let png = image.pngData(), png.count <= request.maxBytes { return (png, image, t) }
      scale *= 0.85
    }
    return nil
  }

  private static func finish(
    _ made: (Data, UIImage, [Double]), region: CGRect, drawing: PKDrawing
  ) -> [String: Any]? {
    let (png, image, t) = made
    let dir = FileManager.default.temporaryDirectory.appendingPathComponent("ink-export")
    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    let url = dir.appendingPathComponent("\(UUID().uuidString).png")
    guard (try? png.write(to: url, options: [.atomic, .completeFileProtection])) != nil else {
      return nil
    }
    let cg = image.cgImage
    return [
      "file": url.path, "mime": "image/png",
      "pixel_width": cg?.width ?? Int(image.size.width),
      "pixel_height": cg?.height ?? Int(image.size.height),
      "bytes": png.count, "sha256": InkHash.sha256(png), "region": InkRegion.json(region),
      "crop_to_document_transform": t,
      "drawing_sha256": InkHash.sha256(drawing.dataRepresentation()),
    ]
  }

  /// A bitmap of [size] units at [scale] pixels each, white underneath.
  private static func render(size: CGSize, scale: CGFloat, draw: (CGContext) -> Void) -> UIImage {
    let format = UIGraphicsImageRendererFormat()
    format.scale = 1
    format.opaque = true
    let pixels = CGSize(width: (size.width * scale).rounded(), height: (size.height * scale).rounded())
    return UIGraphicsImageRenderer(size: pixels, format: format).image { context in
      UIColor.white.setFill()
      context.fill(CGRect(origin: .zero, size: pixels))
      context.cgContext.scaleBy(x: scale, y: scale)
      draw(context.cgContext)
    }
  }

  /// The ink of [rect] at [scale], in light colours: what was drawn, not
  /// what dark mode would make of it.
  private static func ink(_ drawing: PKDrawing, rect: CGRect, scale: CGFloat) -> UIImage {
    var out = UIImage()
    UITraitCollection(userInterfaceStyle: .light).performAsCurrent {
      out = drawing.image(from: rect, scale: scale)
    }
    return out
  }

  /// Turned clockwise by [degrees] (0, 90, 180, 270), as a page is shown.
  private static func rotated(_ image: UIImage, degrees: Int) -> UIImage {
    guard degrees != 0, let cg = image.cgImage else { return image }
    let w = CGFloat(cg.width), h = CGFloat(cg.height)
    let size = degrees == 180 ? CGSize(width: w, height: h) : CGSize(width: h, height: w)
    let format = UIGraphicsImageRendererFormat()
    format.scale = 1
    format.opaque = true
    return UIGraphicsImageRenderer(size: size, format: format).image { context in
      let c = context.cgContext
      c.translateBy(x: size.width / 2, y: size.height / 2)
      c.rotate(by: CGFloat(degrees) * .pi / 180)
      image.draw(in: CGRect(x: -w / 2, y: -h / 2, width: w, height: h))
    }
  }

  /// The affine map [a, b, c, d, tx, ty] (x' = a·u + c·v + tx,
  /// y' = b·u + d·v + ty) of a point function that is affine.
  static func transform(_ f: (CGFloat, CGFloat) -> CGPoint) -> [Double] {
    let o = f(0, 0), x = f(1, 0), y = f(0, 1)
    func r(_ v: CGFloat) -> Double { (Double(v) * 100_000).rounded() / 100_000 }
    return [r(x.x - o.x), r(x.y - o.y), r(y.x - o.x), r(y.y - o.y), r(o.x), r(o.y)]
  }
}
