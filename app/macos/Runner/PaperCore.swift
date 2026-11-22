// The Mac's PDF viewer, what it knows of a PDF (pane kind "pdf"): opening
// the cached copy once its hash is the version the backend named, its
// pages and outline, searching it, regions on a page, and the view's keys.
// No Flutter, so RunnerTests/PaperCheck.swift checks it without the app.
// The same messages as the iPad's ink bridge (app/test/ipad/fixtures/
// bridge), less the ink: the Mac shows PDFs and doesn't draw on them.

import CryptoKit
import Foundation
import PDFKit

enum PaperHash {
  static func sha256(_ data: Data) -> String {
    SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
  }

  /// A file's SHA-256, read in chunks (a PDF may be 64 MiB).
  static func sha256(file url: URL) -> String? {
    guard let handle = try? FileHandle(forReadingFrom: url) else { return nil }
    defer { try? handle.close() }
    var hasher = SHA256()
    do {
      while let chunk = try handle.read(upToCount: 1 << 20), !chunk.isEmpty {
        hasher.update(data: chunk)
      }
    } catch { return nil }
    return hasher.finalize().map { String(format: "%02x", $0) }.joined()
  }
}

enum PaperCore {
  /// Past this a PDF is refused rather than half shown (ipad.md §4).
  static let maxPages = 2000
  /// Outline entries in `ready`, at most.
  static let maxOutline = 500

  /// Opens [path] once its SHA-256 is [expected] (when named): the
  /// document, or the `error` event's fields.
  static func open(path: String, expected: String?) -> Result<(PDFDocument, String), PaperRefusal> {
    let url = URL(fileURLWithPath: path)
    guard let actual = PaperHash.sha256(file: url) else {
      return .failure(PaperRefusal(["code": "document_unreadable", "recoverable": false]))
    }
    if let expected, actual != expected.lowercased() {
      return .failure(
        PaperRefusal([
          "code": "document_hash_mismatch", "recoverable": true, "expected_sha256": expected,
          "actual_sha256": actual,
        ]))
    }
    guard let document = PDFDocument(url: url) else {
      return .failure(PaperRefusal(["code": "document_unreadable", "recoverable": false]))
    }
    // Never unlocked: a password-protected PDF is refused, not bypassed.
    if document.isLocked {
      return .failure(PaperRefusal(["code": "document_locked", "recoverable": false]))
    }
    if document.pageCount > maxPages {
      return .failure(
        PaperRefusal([
          "code": "document_too_large", "recoverable": false, "page_count": document.pageCount,
          "max_pages": maxPages,
        ]))
    }
    return .success((document, actual))
  }

  /// `ready`'s fields: the pages' boxes and turns, and the outline.
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
      "page_count": document.pageCount, "pages": pages, "drawing_space": "none",
    ]
    let entries = outline(document)
    if !entries.isEmpty { out["outline"] = entries }
    return out
  }

  static func box(_ r: CGRect) -> [Double] {
    [r.minX, r.minY, r.width, r.height].map { (Double($0) * 1000).rounded() / 1000 }
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
    guard crop.width > 0, crop.height > 0 else { return .zero }
    return CGRect(
      x: (b.minX - crop.minX) / crop.width, y: (crop.maxY - b.maxY) / crop.height,
      width: b.width / crop.width, height: b.height / crop.height
    ).intersection(CGRect(x: 0, y: 0, width: 1, height: 1))
  }

  static func regionJSON(_ r: CGRect) -> [String: Double] {
    func round5(_ v: CGFloat) -> Double { (Double(v) * 100_000).rounded() / 100_000 }
    return ["x": round5(r.minX), "y": round5(r.minY), "w": round5(r.width), "h": round5(r.height)]
  }

  static func region(_ any: Any?) -> CGRect? {
    guard let m = any as? [String: Any],
      let x = (m["x"] as? NSNumber)?.doubleValue, let y = (m["y"] as? NSNumber)?.doubleValue,
      let w = (m["w"] as? NSNumber)?.doubleValue, let h = (m["h"] as? NSNumber)?.doubleValue
    else { return nil }
    return CGRect(x: x, y: y, width: w, height: h)
  }

  struct Match {
    /// 0-based.
    let page: Int
    let range: NSRange
    let region: CGRect
    /// The line it's on.
    let text: String
  }

  /// [query]'s matches in [document], at most [limit], each with its line.
  static func search(_ document: PDFDocument, query: String, limit: Int) -> (
    matches: [Match], truncated: Bool
  ) {
    var matches: [Match] = []
    for selection in document.findString(query, withOptions: [.caseInsensitive, .diacriticInsensitive]) {
      guard let page = selection.pages.first else { continue }
      if matches.count == limit { return (matches, true) }
      let line = selection.copy() as? PDFSelection ?? selection
      line.extendForLineBoundaries()
      matches.append(
        Match(
          page: document.index(for: page), range: selection.range(at: 0, on: page),
          region: region(selection.bounds(for: page), on: page),
          text: (line.string ?? query).trimmingCharacters(in: .whitespacesAndNewlines)))
    }
    return (matches, false)
  }

  /// The answer to `search`, as the fixture has it (cmd_search.json).
  static func searchJSON(query: String, matches: [Match], truncated: Bool) -> [String: Any] {
    [
      "query": query, "truncated": truncated,
      "matches": matches.map {
        ["page_1based": $0.page + 1, "region": regionJSON($0.region), "text": $0.text] as [String: Any]
      },
    ]
  }

  /// Form fields show what they hold and are never filled in or submitted.
  static func readOnlyForms(_ document: PDFDocument) {
    for index in 0..<document.pageCount {
      for annotation in document.page(at: index)?.annotations ?? [] where annotation.type == "Widget" {
        annotation.isReadOnly = true
      }
    }
  }
}

/// Why a PDF isn't shown: the `error` event's fields.
struct PaperRefusal: Error {
  let fields: [String: Any]
  init(_ fields: [String: Any]) { self.fields = fields }
  var code: String { fields["code"] as? String ?? "" }
}

/// PDFKit's view, with ⌘F, ⌘G and ⇧⌘G handed to Dart's find field. AppKit
/// offers a key equivalent to every view in the window before the first
/// responder's keyDown, so they're taken only while the page has the keys:
/// ⌘F in a message box beside it is the box's.
final class PaperPDFView: PDFView {
  var onFind: ((_ forward: Bool?) -> Void)?

  override func performKeyEquivalent(with event: NSEvent) -> Bool {
    guard let responder = window?.firstResponder as? NSView, responder.isDescendant(of: self) else {
      return super.performKeyEquivalent(with: event)
    }
    let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
    if flags.contains(.command), let key = event.charactersIgnoringModifiers?.lowercased() {
      switch key {
      case "f" where !flags.contains(.shift):
        onFind?(nil)
        return true
      case "g":
        onFind?(!flags.contains(.shift))
        return true
      default: break
      }
    }
    return super.performKeyEquivalent(with: event)
  }
}
