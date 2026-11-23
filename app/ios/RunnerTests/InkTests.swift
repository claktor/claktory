import Flutter
import ImageIO
import PDFKit
import PencilKit
import Testing
import UIKit

@testable import Runner

/// The ink bridge's native side (app/ios/Runner/Ink) against the frozen
/// fixtures in app/test/ipad/fixtures/bridge, read by #filePath.
// Serialized: some tests post app-wide notifications (background, memory)
// that every live surface hears.
@Suite(.serialized)
@MainActor
struct InkTests {
  // MARK: Fixtures

  static let fixtures = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    .appendingPathComponent("test/ipad/fixtures")

  static func fixture(_ path: String) throws -> [String: Any] {
    let data = try Data(contentsOf: fixtures.appendingPathComponent(path))
    return try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
  }

  static func arguments(_ path: String) throws -> [String: Any] {
    try #require(try fixture(path)["arguments"] as? [String: Any])
  }

  // MARK: Helpers

  /// Stands in for the surface's method channel.
  final class Events: InkEvents {
    var handler: FlutterMethodCallHandler?
    var sent: [(String, [String: Any])] = []

    func invokeMethod(_ method: String, arguments: Any?) {
      sent.append((method, arguments as? [String: Any] ?? [:]))
    }

    func setMethodCallHandler(_ handler: FlutterMethodCallHandler?) {
      self.handler = handler
    }

    func call(_ method: String, _ arguments: Any? = nil) async -> Any? {
      await withCheckedContinuation { done in
        // As Flutter calls it: on the main thread.
        DispatchQueue.main.async {
          self.handler!(FlutterMethodCall(methodName: method, arguments: arguments)) {
            done.resume(returning: $0)
          }
        }
      }
    }

    func named(_ method: String) -> [[String: Any]] {
      sent.filter { $0.0 == method }.map(\.1)
    }
  }

  /// A surface in a window, as the app shows it, attached.
  static func surface(_ args: [String: Any], size: CGSize = CGSize(width: 800, height: 1000))
    async -> (InkSurface, Events, UIWindow)
  {
    let events = Events()
    let surface = InkFactory.surface(
      frame: CGRect(origin: .zero, size: size), params: InkParams(args), channel: events)
    let window = UIWindow(frame: CGRect(origin: .zero, size: size))
    window.addSubview(surface.view())
    window.isHidden = false
    window.layoutIfNeeded()
    _ = await events.call("attach")
    return (surface, events, window)
  }

  static func settle(_ seconds: Double = 0.4) async {
    try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
  }

  /// A paper whose pages are as the fixtures' are: Letter, page 5 turned
  /// 90°, page 7 cropped to [72, 144, 432, 504].
  /// Six pages with an outline as LaTeX writes one: entries with /Dest and
  /// with a /GoTo action, one nested (PDFKit doesn't write an outline set
  /// in code, so it's written here by hand).
  static func outlined() throws -> (URL, String) {
    var objects: [String] = []
    func add(_ o: String) -> Int {
      objects.append(o)
      return objects.count
    }
    _ = add("<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R >>")
    _ = add("PAGES")
    _ = add("OUTLINES")
    let pages = (1...6).map { _ in add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>") }
    objects[1] = "<< /Type /Pages /Kids [\(pages.map { "\($0) 0 R" }.joined(separator: " "))] /Count 6 >>"
    // 1 Introduction → p1 (/Dest), 2 Method → p3 (/A GoTo) with child
    // 2.1 Gradients → p5, 3 Results → p5.
    let intro = add("INTRO"), method = add("METHOD"), grads = add("GRADS"), results = add("RESULTS")
    objects[intro - 1] =
      "<< /Title (Introduction) /Parent 3 0 R /Next \(method) 0 R /Dest [\(pages[0]) 0 R /Fit] >>"
    objects[method - 1] =
      "<< /Title (Method) /Parent 3 0 R /Prev \(intro) 0 R /Next \(results) 0 R /First \(grads) 0 R /Last \(grads) 0 R /Count 1 /A << /S /GoTo /D [\(pages[2]) 0 R /Fit] >> >>"
    objects[grads - 1] =
      "<< /Title (Gradients) /Parent \(method) 0 R /Dest [\(pages[4]) 0 R /XYZ 0 792 0] >>"
    objects[results - 1] =
      "<< /Title (Results) /Parent 3 0 R /Prev \(method) 0 R /Dest [\(pages[4]) 0 R /Fit] >>"
    objects[2] = "<< /Type /Outlines /First \(intro) 0 R /Last \(results) 0 R /Count 4 >>"
    var out = "%PDF-1.4\n"
    var offsets: [Int] = []
    for (i, o) in objects.enumerated() {
      offsets.append(out.utf8.count)
      out += "\(i + 1) 0 obj\n\(o)\nendobj\n"
    }
    let xref = out.utf8.count
    out += "xref\n0 \(objects.count + 1)\n0000000000 65535 f \n"
    for o in offsets { out += String(format: "%010d 00000 n \n", o) }
    out += "trailer\n<< /Size \(objects.count + 1) /Root 1 0 R >>\nstartxref\n\(xref)\n%%EOF\n"
    let url = FileManager.default.temporaryDirectory.appendingPathComponent("ink-\(UUID()).pdf")
    try Data(out.utf8).write(to: url)
    return (url, try #require(InkHash.sha256(file: url)))
  }

  /// `edited`: pages whose sentence says something else (a new version).
  static func paper(pages: Int = 8, edited: Set<Int> = []) throws -> (URL, String) {
    let bounds = CGRect(x: 0, y: 0, width: 612, height: 792)
    let data = UIGraphicsPDFRenderer(bounds: bounds).pdfData { context in
      for n in 1...pages {
        context.beginPage()
        guard pages <= 9 else { continue }
        ("Page \(n)" as NSString).draw(
          at: CGPoint(x: 72, y: 72), withAttributes: [.font: UIFont.systemFont(ofSize: 24)])
        // Something to find, 100 pt from the top, on pages 3 and 5.
        if n == 3 || n == 5 {
          ((edited.contains(n) ? "the exploding gradient" : "the vanishing gradient") as NSString).draw(
            at: CGPoint(x: 72, y: 100), withAttributes: [.font: UIFont.systemFont(ofSize: 14)])
        }
      }
    }
    let doc = try #require(PDFDocument(data: data))
    if pages >= 7 {
      doc.page(at: 4)?.rotation = 90
      doc.page(at: 6)?.setBounds(CGRect(x: 72, y: 144, width: 432, height: 504), for: .cropBox)
    }
    // A form field on page 1, to be shown and never filled in.
    let field = PDFAnnotation(
      bounds: CGRect(x: 72, y: 600, width: 200, height: 24), forType: .widget, withProperties: nil)
    field.widgetFieldType = .text
    field.fieldName = "name"
    doc.page(at: 0)?.addAnnotation(field)
    let url = FileManager.default.temporaryDirectory.appendingPathComponent("ink-\(UUID()).pdf")
    #expect(doc.write(to: url))
    return (url, try #require(InkHash.sha256(file: url)))
  }

  static func pdfArgs(file: URL, sha: String) throws -> [String: Any] {
    var args = try arguments("bridge/create_pdf.json")
    var source = try #require(args["source"] as? [String: Any])
    source["sha256"] = sha
    args["source"] = source
    args["document"] = ["file": file.path, "sha256": sha]
    args["initial"] = ["page_1based": 1]
    return args
  }

  static func circle(at c: CGPoint, radius: CGFloat = 20) -> PKStroke {
    let points = (0...24).map { i -> PKStrokePoint in
      let a = CGFloat(i) / 24 * 2 * .pi
      return PKStrokePoint(
        location: CGPoint(x: c.x + radius * cos(a), y: c.y + radius * sin(a)),
        timeOffset: Double(i) * 0.01, size: CGSize(width: 3, height: 3), opacity: 1, force: 1,
        azimuth: 0, altitude: .pi / 2)
    }
    return PKStroke(
      ink: PKInk(.pen, color: .black),
      path: PKStrokePath(controlPoints: points, creationDate: Date()))
  }

  static func keys(_ m: [String: Any]) -> Set<String> { Set(m.keys) }

  // MARK: Creation

  @Test func paramsReadTheFixtures() throws {
    let pdf = InkParams(try Self.arguments("bridge/create_pdf.json"))
    #expect(pdf.surface == "pdf")
    #expect(pdf.leafId == "leaf-7f3a")
    #expect(pdf.layers.count == 3)
    #expect(pdf.initialPage == 3)
    #expect(pdf.drawingPolicy == PKCanvasViewDrawingPolicy.default)
    #expect(pdf.style == UIUserInterfaceStyle.light)
    let image = InkParams(try Self.arguments("bridge/create_image.json"))
    #expect(image.drawingPolicy == PKCanvasViewDrawingPolicy.anyInput)
  }

  // MARK: PDF

  @available(iOS 16.0, *)
  @Test func aPaperIsReadyWithItsPagesAsTheFixtureSays() async throws {
    let (file, sha) = try Self.paper()
    let (_, events, window) = await Self.surface(try Self.pdfArgs(file: file, sha: sha))
    await Self.settle()
    let ready = try #require(events.named("ready").first)
    let fixture = try Self.arguments("bridge/ev_ready.json")
    #expect(Self.keys(ready) == Self.keys(fixture).union(["handwriting"]))
    #expect(ready["sha256_verified"] as? Bool == true)
    #expect(ready["page_count"] as? Int == 8)
    let pages = try #require(ready["pages"] as? [[String: Any]])
    #expect(Self.keys(pages[0]) == Self.keys((fixture["pages"] as! [[String: Any]])[0]))
    #expect(pages[4]["rotation"] as? Int == 90)
    #expect(pages[6]["crop_box"] as? [Double] == [72, 144, 432, 504])
    let page = try #require(events.named("pageChanged").last)
    #expect(Self.keys(page) == Self.keys(try Self.arguments("bridge/ev_page_changed.json")))
    _ = window
  }

  /// In a window as narrow as Slide Over, two whole pages show at once:
  /// the header names the first, not the one across the middle.
  @available(iOS 16.0, *)
  @Test func aTallNarrowWindowNamesThePageShownMost() async throws {
    let (file, sha) = try Self.paper(pages: 5)
    let (surface, events, window) = await Self.surface(
      try Self.pdfArgs(file: file, sha: sha), size: CGSize(width: 375, height: 1100))
    await Self.settle()
    let paper = try #require(surface as? PaperView)
    let doc = try #require(paper.pdf.document)
    #expect(paper.pdf.visiblePages.count >= 2)
    #expect(events.named("pageChanged").last?["page_1based"] as? Int == 1)
    #expect(paper.statusText?.label == "Page 1 of 5")

    // Scrolled so most of page 2 shows, with no page-changed note of
    // PDFKit's needed.
    let scroll = try #require(paper.pdf.subviews.compactMap { $0 as? UIScrollView }.first)
    let two = try #require(doc.page(at: 1))
    let top = paper.pdf.convert(two.bounds(for: paper.pdf.displayBox), from: two).minY
    var offset = scroll.contentOffset
    offset.y += top - 100
    scroll.setContentOffset(offset, animated: false)
    await Self.settle(0.2)
    #expect(PaperView.mostShown(in: paper.pdf).map { doc.index(for: $0) } == 1)
    #expect(events.named("pageChanged").last?["page_1based"] as? Int == 2)
    #expect(paper.statusText?.label == "Page 2 of 5")
    // Sent once for the move, however many offsets it went through.
    let sent = events.named("pageChanged").count
    scroll.setContentOffset(CGPoint(x: offset.x, y: offset.y + 5), animated: false)
    await Self.settle(0.2)
    #expect(events.named("pageChanged").count == sent)
    // Scrolled to the end: the last page, though page 4 is whole too.
    let inset = scroll.adjustedContentInset
    let end = scroll.contentSize.height - scroll.bounds.height + inset.bottom
    scroll.setContentOffset(CGPoint(x: offset.x, y: end), animated: false)
    await Self.settle(0.2)
    #expect(events.named("pageChanged").last?["page_1based"] as? Int == 5)
    #expect(paper.statusText?.label == "Page 5 of 5")
    // A little way up from the end: the page shown most again.
    scroll.setContentOffset(CGPoint(x: offset.x, y: end - 40), animated: false)
    await Self.settle(0.2)
    #expect(events.named("pageChanged").last?["page_1based"] as? Int == 4)
    _ = window
  }

  /// On an iPhone's screen the first page partly scrolled away: the header
  /// follows whichever page has more of it in view.
  @available(iOS 16.0, *)
  @Test func aPhoneNamesThePageMostlyInView() async throws {
    let (file, sha) = try Self.paper(pages: 5)
    let (surface, events, window) = await Self.surface(
      try Self.pdfArgs(file: file, sha: sha), size: CGSize(width: 390, height: 844))
    await Self.settle()
    let paper = try #require(surface as? PaperView)
    let doc = try #require(paper.pdf.document)
    let scroll = try #require(paper.pdf.subviews.compactMap { $0 as? UIScrollView }.first)
    let one = try #require(doc.page(at: 0))
    let height = paper.pdf.convert(one.bounds(for: paper.pdf.displayBox), from: one).height
    let start = scroll.contentOffset
    func scrolled(by fraction: CGFloat) async -> Int? {
      scroll.setContentOffset(CGPoint(x: start.x, y: start.y + height * fraction), animated: false)
      await Self.settle(0.2)
      return events.named("pageChanged").last?["page_1based"] as? Int
    }
    // A tenth of page 1 scrolled away: most of what shows is still page 1.
    #expect(await scrolled(by: 0.1) == 1)
    // Two thirds of it gone: page 2 has more of the screen.
    #expect(await scrolled(by: 0.67) == 2)
    #expect(paper.statusText?.label == "Page 2 of 5")
    // Back: page 1 again.
    #expect(await scrolled(by: 0.05) == 1)
    _ = window
  }

  @available(iOS 16.0, *)
  @Test func aChangedPaperIsRefused() async throws {
    let (file, _) = try Self.paper()
    let other = String(repeating: "a", count: 64)
    let (_, events, window) = await Self.surface(try Self.pdfArgs(file: file, sha: other))
    await Self.settle()
    #expect(events.named("ready").isEmpty)
    let error = try #require(events.named("error").first)
    #expect(error["code"] as? String == "document_hash_mismatch")
    #expect(error["expected_sha256"] as? String == other)
    _ = window
  }

  @available(iOS 16.0, *)
  @Test func pageInkIsInCropPointsFromTheTopLeft() async throws {
    // drawing_to_document = [1, 0, 0, -1, cx, cy + ch], as the fixtures
    // keep it, checked through PDFKit's own conversions.
    let (file, sha) = try Self.paper()
    let (surface, _, window) = await Self.surface(try Self.pdfArgs(file: file, sha: sha))
    await Self.settle()
    let paper = try #require(surface as? PaperView)
    for (n, crop) in [(1, CGRect(x: 0, y: 0, width: 612, height: 792)), (7, CGRect(x: 72, y: 144, width: 432, height: 504))] {
      let page = try #require(paper.pdf.document?.page(at: n - 1))
      paper.pdf.go(to: page)
      await Self.settle(0.3)
      let canvas = try #require(paper.liveCanvases.first { $0.tag == n })
      func toPage(_ p: CGPoint) -> CGPoint {
        paper.pdf.convert(canvas.convert(p, to: paper.pdf), to: page)
      }
      let origin = toPage(.zero)
      let corner = toPage(CGPoint(x: crop.width, y: crop.height))
      #expect(abs(origin.x - crop.minX) < 0.5 && abs(origin.y - crop.maxY) < 0.5)
      #expect(abs(corner.x - crop.maxX) < 0.5 && abs(corner.y - crop.minY) < 0.5)
    }
    _ = window
  }

  // MARK: PDFs, phase 3

  @available(iOS 16.0, *)
  static func openPaper(pages: Int = 8, outline: Bool = false) async throws
    -> (PaperView, Events, UIWindow)
  {
    let (file, sha) = try outline ? outlined() : paper(pages: pages)
    let (surface, events, window) = await surface(try pdfArgs(file: file, sha: sha))
    await settle()
    return (try #require(surface as? PaperView), events, window)
  }

  @available(iOS 16.0, *)
  @Test func theOutlineComesWithReady() async throws {
    let (paper, events, window) = try await Self.openPaper(outline: true)
    let ready = try #require(events.named("ready").first)
    let outline = try #require(ready["outline"] as? [[String: Any]])
    #expect(outline.map { $0["title"] as? String } == ["Introduction", "Method", "Gradients", "Results"])
    #expect(outline.map { $0["page_1based"] as? Int } == [1, 3, 5, 5])
    #expect(outline.map { $0["depth"] as? Int } == [0, 0, 1, 0])
    let fixture = try Self.arguments("bridge/ev_ready_outline.json")
    #expect(Self.keys(ready) == Self.keys(fixture).union(["handwriting"]))
    #expect(
      Self.keys(outline[0]) == Self.keys((fixture["outline"] as! [[String: Any]])[0]))
    // No outline: no key.
    let (other, plain, otherWindow) = try await Self.openPaper()
    #expect(plain.named("ready").first?["outline"] == nil)
    _ = (paper, window, other, otherWindow)
  }

  @available(iOS 16.0, *)
  @Test func searchFindsTextAsRegionsOnItsPages() async throws {
    let (paper, events, window) = try await Self.openPaper()
    let answer = try #require(
      await events.call("search", ["query": "Vanishing Gradient", "max_results": 200])
        as? [String: Any])
    let fixture = try #require(try Self.fixture("bridge/cmd_search.json")["result"] as? [String: Any])
    #expect(Self.keys(answer) == Self.keys(fixture))
    #expect(answer["query"] as? String == "Vanishing Gradient")
    let matches = try #require(answer["matches"] as? [[String: Any]])
    #expect(matches.map { $0["page_1based"] as? Int } == [3, 5])
    for match in matches {
      #expect((match["text"] as? String)?.lowercased().contains("vanishing gradient") == true)
      let r = try #require(InkRegion.rect(match["region"]))
      // "the vanishing gradient" is drawn 72 pt from the left, ~100 pt
      // from the top of a 612 × 792 page; the match starts after "the ".
      // Page 5's /Rotate 90 notwithstanding: the unrotated CropBox.
      #expect(r.minX > 72.0 / 612 && r.minX < 110.0 / 612, "\(match)")
      #expect(abs(r.minY - 100.0 / 792) < 0.03, "\(match)")
    }
    #expect(paper.found.values.reduce(0) { $0 + $1.count } == 2)
    #expect(Set(paper.found.keys) == [3, 5])
    // At most what was asked, and said so.
    let one = await events.call("search", ["query": "gradient", "max_results": 1]) as? [String: Any]
    #expect((one?["matches"] as? [[String: Any]])?.count == 1)
    #expect(one?["truncated"] as? Bool == true)
    // An empty query clears.
    let none = await events.call("search", ["query": " "]) as? [String: Any]
    #expect((none?["matches"] as? [Any])?.isEmpty == true)
    #expect(paper.found.isEmpty)
    _ = window
  }

  @available(iOS 16.0, *)
  @Test func aSearchWashesItsMatchesOnThePagesAndClearingTakesThemOff() async throws {
    // As an iPhone shows it: nothing draws (Zucchero's row 75 was there).
    let (paper, events, window) = try await Self.openPaper()
    _ = await events.call("setDrawingPolicy", ["drawing_policy": "none"])
    _ = await events.call("goToPage", ["page_1based": 3])
    await Self.settle(0.5)
    let canvas = try #require(paper.liveCanvases.first { $0.tag == 3 })
    _ = await events.call("search", ["query": "gradient", "max_results": 200])
    // Page 3's "the vanishing gradient", 72 pt in, ~100 pt from the top.
    let rect = try #require(canvas.found.first)
    #expect(canvas.found.count == 1)
    #expect(rect.minX > 100 && rect.minX < 200 && abs(rect.minY - 90) < 20, "\(rect)")
    #expect(canvas.foundLayer?.superlayer === canvas.layer)
    // Not PDFKit's own, which drew red and stayed.
    #expect(paper.pdf.highlightedSelections?.isEmpty ?? true)
    // Another search shows only its own.
    _ = await events.call("search", ["query": "page 3", "max_results": 200])
    #expect(canvas.found.count == 1 && canvas.found.first != rect)
    // Going to a match marks it apart and brings it into the view's upper
    // part, clear of a find bar and keyboard below (Zucchero's row 91).
    let answer = try #require(
      await events.call("search", ["query": "vanishing gradient", "max_results": 200])
        as? [String: Any])
    let last = try #require((answer["matches"] as? [[String: Any]])?.last)
    #expect(last["page_1based"] as? Int == 5)
    _ = await events.call(
      "goToPage", ["page_1based": 5, "region": last["region"]!, "match": true])
    await Self.settle(0.3)
    let five = try #require(paper.liveCanvases.first { $0.tag == 5 })
    #expect(five.current != nil && five.currentLayer != nil)
    #expect(paper.liveCanvases.filter { $0.current != nil }.count == 1)
    let page5 = try #require(paper.pdf.document?.page(at: 4))
    let shown = paper.pdf.convert(
      PaperView.pageRect(try #require(InkRegion.rect(last["region"])), on: page5), from: page5)
    let height = paper.pdf.bounds.height
    #expect(shown.minY >= 0 && shown.maxY <= height * 0.5, "\(shown) in \(height)")
    // The header names the match's page until the person scrolls or goes
    // elsewhere (Zucchero's row 137).
    #expect(paper.matchPage == 5 && paper.statusText?.page == 5)
    _ = await events.call("goToPage", ["page_1based": 3])
    await Self.settle(0.3)
    #expect(paper.matchPage == nil && paper.statusText?.page == 3)
    _ = await events.call(
      "goToPage", ["page_1based": 5, "region": last["region"]!, "match": true])
    // Cleared: nothing on any page.
    _ = await events.call("search", ["query": ""])
    #expect(paper.liveCanvases.allSatisfy { $0.current == nil })
    #expect(paper.found.isEmpty)
    #expect(paper.liveCanvases.allSatisfy { $0.found.isEmpty && $0.foundLayer == nil })
    _ = window
  }

  /// A report as macOS prints one: a block of monospaced lines, each
  /// starting "Line N of page P:" (Zucchero's field-guide.pdf, row 117),
  /// where PDFPage.characterBounds lands a line away.
  static func report() throws -> PDFDocument {
    let lines = (1...40).map { "Line \($0) of page 1: notes on birds, trees and weather near the river." }
    let data = UIGraphicsPDFRenderer(bounds: CGRect(x: 0, y: 0, width: 612, height: 792)).pdfData {
      context in
      context.beginPage()
      (lines.joined(separator: "\n") as NSString).draw(
        in: CGRect(x: 54, y: 54, width: 504, height: 700),
        withAttributes: [.font: UIFont(name: "Menlo-Regular", size: 12)!])
    }
    return try #require(PDFDocument(data: data))
  }

  @available(iOS 16.0, *)
  @Test func aMatchsBoundsAreItsOwnCharacters() throws {
    let doc = try Self.report()
    let page = try #require(doc.page(at: 0))
    let advance = ("M" as NSString).size(
      withAttributes: [.font: UIFont(name: "Menlo-Regular", size: 12)!]
    ).width
    for (query, column) in [("Line 30 of page", 0), ("weather", 44), ("Line 3 of", 0)] {
      let selection = try #require(doc.findString(query, withOptions: [.caseInsensitive]).first)
      let r = PaperView.tightBounds(of: selection, on: page)
      let width = CGFloat(query.count) * advance
      #expect(abs(r.minX - (54 + CGFloat(column) * advance)) < advance / 3, "\(query): \(r)")
      #expect(abs(r.width - width) < advance / 2, "\(query): \(r) for \(width)")
      #expect(r.height > 8 && r.height < 20, "\(query): \(r)")
    }
  }

  /// A macOS print whose lines' first few characters PDFKit places far
  /// too wide, each as tall as the page (Zucchero's field-guide.pdf, row
  /// 117: a match starting a line was washed from the margin to past its
  /// end). Its text is Menlo-like, 7.2 pt a character, from 16.6 pt in.
  @available(iOS 16.0, *)
  @Test func aMatchStartingALineIsWashedFromItsOwnStart() throws {
    let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .appendingPathComponent("fixtures/field-guide.pdf")
    let doc = try #require(PDFDocument(url: url))
    for (query, column) in [("Line 30 of page", 0), ("Line 3 of", 0), ("weather", 44)] {
      for selection in doc.findString(query, withOptions: [.caseInsensitive]).prefix(3) {
        let page = try #require(selection.pages.first)
        let r = PaperView.tightBounds(of: selection, on: page)
        #expect(abs(r.minX - (16.6 + CGFloat(column) * 7.2)) < 2, "\(query): \(r)")
        #expect(abs(r.width - CGFloat(query.count) * 7.2) < 3, "\(query): \(r)")
        #expect(r.height > 8 && r.height < 24, "\(query): \(r)")
      }
    }
  }

  @available(iOS 16.0, *)
  @Test func anOlderSearchIsSuperseded() async throws {
    // Kept, as Flutter keeps the platform view: a freed surface answers nothing.
    let (paper, events, window) = try await Self.openPaper()
    // Both asked before either is answered, as typing does.
    var answers: [String: [String: Any]] = [:]
    await withCheckedContinuation { (done: CheckedContinuation<Void, Never>) in
      var left = 2
      for query in ["gradient", "vanishing"] {
        events.handler!(FlutterMethodCall(methodName: "search", arguments: ["query": query])) {
          answers[query] = $0 as? [String: Any]
          left -= 1
          if left == 0 { done.resume() }
        }
      }
    }
    let (a, b) = (answers["gradient"], answers["vanishing"])
    #expect(a?["superseded"] as? Bool == true)
    #expect(a?["query"] as? String == "gradient")
    #expect(b?["superseded"] == nil)
    #expect((b?["matches"] as? [Any])?.count == 2)
    _ = (paper, window)
  }

  @available(iOS 16.0, *)
  @Test func goToPageWithARegionAndLinksThatOnlyTell() async throws {
    let (paper, events, window) = try await Self.openPaper()
    let args = try Self.arguments("bridge/cmd_go_to_page_region.json")
    let answer = await events.call("goToPage", args) as? [String: Any]
    await Self.settle(0.3)
    #expect(answer?["page_1based"] as? Int == 5)
    // Region ↔ page space both ways.
    let page = try #require(paper.pdf.document?.page(at: 6))
    let r = CGRect(x: 0.1, y: 0.2, width: 0.3, height: 0.4)
    let back = PaperView.region(PaperView.pageRect(r, on: page), on: page)
    #expect(abs(back.minX - r.minX) < 1e-9 && abs(back.minY - r.minY) < 1e-9)
    #expect(abs(back.width - r.width) < 1e-9 && abs(back.height - r.height) < 1e-9)
    // A link out of the PDF is only reported.
    paper.pdfViewWillClick(onLink: paper.pdf, with: URL(string: "https://arxiv.org/abs/1211.5063")!)
    let link = try #require(events.named("linkTapped").first)
    #expect(Self.keys(link) == Self.keys(try Self.arguments("bridge/ev_link_tapped.json")))
    #expect(link["url"] as? String == "https://arxiv.org/abs/1211.5063")
    _ = window
  }

  @available(iOS 16.0, *)
  @Test func formFieldsAreReadOnlyAndAWholePageExports() async throws {
    let (paper, events, window) = try await Self.openPaper()
    let page = try #require(paper.pdf.document?.page(at: 0))
    _ = paper.pdfView(paper.pdf, overlayViewFor: page)
    let fields = page.annotations.filter { $0.type == "Widget" }
    #expect(!fields.isEmpty)
    #expect(fields.allSatisfy { $0.isReadOnly })
    let out = try #require(
      await events.call("exportComposite", ["page_1based": 1, "max_long_edge": 1024]) as? [String: Any])
    #expect(InkRegion.rect(out["region"]) == CGRect(x: 0, y: 0, width: 1, height: 1))
    #expect(out["pixel_height"] as? Int == 1024)
    _ = window
  }

  @available(iOS 16.0, *)
  @Test func drawingOffLeavesThePageToPDFKit() async throws {
    let (paper, events, window) = try await Self.openPaper()
    let page = try #require(paper.pdf.document?.page(at: 0))
    let canvas = try #require(paper.pdfView(paper.pdf, overlayViewFor: page) as? InkCanvas)
    #expect(canvas.isUserInteractionEnabled)
    // Drawing on: the page's touches are PencilKit's.
    #expect(paper.pdf.isInMarkupMode)
    paper.canvasViewDidBeginUsingTool(canvas)
    let args = try Self.arguments("bridge/cmd_set_drawing_policy_none.json")
    let off = await events.call("setDrawingPolicy", args) as? [String: Any]
    #expect(off?["drawing_policy"] as? String == "none")
    // Off: PDFKit selects text and answers double-taps again.
    #expect(!paper.pdf.isInMarkupMode)
    #expect(!canvas.isUserInteractionEnabled && !canvas.drawingGestureRecognizer.isEnabled)
    #expect(events.named("strokeInterrupted").first?["reason"] as? String == "drawing_off")
    // A page shown later is off too; turned on again, every page draws.
    let later = try #require(paper.pdf.document?.page(at: 3))
    let next = try #require(paper.pdfView(paper.pdf, overlayViewFor: later) as? InkCanvas)
    #expect(!next.isUserInteractionEnabled)
    _ = await events.call("setDrawingPolicy", ["drawing_policy": "pencil_only"])
    #expect(canvas.isUserInteractionEnabled && canvas.drawingGestureRecognizer.isEnabled)
    #expect(canvas.drawingPolicy == PKCanvasViewDrawingPolicy.pencilOnly)
    #expect(paper.pdf.isInMarkupMode)
    _ = window
  }

  @Test func drawingOffOnAnImageStillScrolls() async throws {
    var args = try Self.arguments("bridge/create_blank.json")
    args["drawing_policy"] = "none"
    let (surface, _, window) = await Self.surface(args)
    let canvas = try #require(surface.liveCanvases.first)
    #expect(!canvas.drawingGestureRecognizer.isEnabled)
    #expect(canvas.isUserInteractionEnabled && canvas.isScrollEnabled)
    _ = window
  }

  @available(iOS 16.0, *)
  @Test func moreThan2000PagesIsRefused() async throws {
    let (_, events, window) = try await Self.openPaper(pages: 2001)
    #expect(events.named("ready").isEmpty)
    let error = try #require(events.named("error").first)
    let fixture = try Self.arguments("bridge/ev_error_document_too_large.json")
    #expect(Self.keys(error) == Self.keys(fixture))
    #expect(error["code"] as? String == "document_too_large")
    #expect(error["page_count"] as? Int == 2001)
    #expect(error["max_pages"] as? Int == 2000)
    _ = window
  }

  // MARK: Images

  @Test func anImageLoadedBeforeItsFirstLayoutFillsItsView() async throws {
    let png = UIGraphicsImageRenderer(size: CGSize(width: 600, height: 400)).pngData { context in
      UIColor.orange.setFill()
      context.fill(CGRect(x: 0, y: 0, width: 600, height: 400))
    }
    let url = FileManager.default.temporaryDirectory.appendingPathComponent("ink-\(UUID()).png")
    try png.write(to: url)
    var args = try Self.arguments("bridge/create_image.json")
    let sha = InkHash.sha256(png)
    args["document"] = ["file": url.path, "sha256": sha]
    var source = try #require(args["source"] as? [String: Any])
    source["sha256"] = sha
    args["source"] = source
    // As Flutter makes it: no size until its first layout.
    let events = Events()
    let surface = InkFactory.surface(frame: .zero, params: InkParams(args), channel: events)
    _ = await events.call("attach")
    await Self.settle()
    let ready = try #require(events.named("ready").first)
    #expect(ready["drawing_space"] as? String == "oriented_image")
    let window = UIWindow(frame: CGRect(x: 0, y: 0, width: 800, height: 1000))
    window.addSubview(surface.view())
    window.isHidden = false
    surface.view().frame = window.bounds
    surface.view().layoutIfNeeded()
    let canvas = try #require(surface.liveCanvases.first)
    // 600 x 400 pixels at the scale the image is read at, fitted to 800 wide.
    #expect(abs(canvas.contentSize.width - 800) < 1)
    #expect(abs(canvas.contentSize.height - 800 * 400 / 600) < 1)
    #expect(events.named("strokeInterrupted").isEmpty)
  }

  // MARK: Changes

  @Test func aStrokeIsSentOnceAsTheFixtureSays() async throws {
    let (surface, events, window) = await Self.surface(try Self.arguments("bridge/create_blank.json"))
    await Self.settle()
    let canvas = try #require(surface.liveCanvases.first)
    canvas.drawing = PKDrawing(strokes: [Self.circle(at: CGPoint(x: 100, y: 100))])
    canvas.drawing = PKDrawing(strokes: canvas.drawing.strokes + [Self.circle(at: CGPoint(x: 500, y: 300))])
    await Self.settle()
    let changes = events.named("drawingChanged")
    #expect(changes.count == 1)  // coalesced
    let change = try #require(changes.first)
    #expect(Self.keys(change) == Self.keys(try Self.arguments("bridge/ev_drawing_changed.json")))
    #expect(change["stroke_count"] as? Int == 2)
    #expect(change["ink_layer_id"] is NSNull)
    #expect(change["base_revision"] is NSNull)
    #expect(change["cause"] as? String == "stroke")
    let bounds = try #require(InkRegion.rect(change["bounds"]))
    // Normalized to the 1024×768 sketch.
    #expect(abs(bounds.minX - 78.0 / 1024) < 0.01)
    let drawing = try #require(change["drawing"] as? [String: Any])
    let data = try #require(Data(base64Encoded: drawing["bytes_base64"] as! String))
    #expect(drawing["sha256"] as? String == InkHash.sha256(data))
    #expect(try PKDrawing(data: data).strokes.count == 2)

    // Saved: the next change names its layer and revision, and only the
    // new stroke is new.
    _ = await events.call("layerSaved", ["page_1based": NSNull(), "ink_layer_id": "layer-9", "revision": 1])
    canvas.drawing = PKDrawing(strokes: canvas.drawing.strokes + [Self.circle(at: CGPoint(x: 800, y: 600))])
    await Self.settle()
    let next = try #require(events.named("drawingChanged").last)
    #expect(next["ink_layer_id"] as? String == "layer-9")
    #expect(next["base_revision"] as? Int == 1)
    let added = try #require(InkRegion.rect(next["new_bounds"]))
    #expect(added.minX > 0.7)
    _ = window
  }

  @Test func loadingADrawingIsNotAChange() async throws {
    let (surface, events, window) = await Self.surface(try Self.arguments("bridge/create_blank.json"))
    let drawing = PKDrawing(strokes: [Self.circle(at: CGPoint(x: 50, y: 50))])
    let data = drawing.dataRepresentation()
    let result = await events.call(
      "loadDrawing",
      [
        "layers": [
          [
            "page_1based": NSNull(), "ink_layer_id": "layer-3", "revision": 4,
            "drawing": [
              "format": "pencilkit", "content_version": 1,
              "bytes_base64": data.base64EncodedString(), "sha256": InkHash.sha256(data),
            ],
          ],
          [
            "page_1based": 2, "ink_layer_id": "layer-4", "revision": 1,
            "drawing": [
              "format": "pencilkit", "content_version": 1, "bytes_base64": "AAAA",
              "sha256": String(repeating: "0", count: 64),
            ],
          ],
        ]
      ]) as? [String: Any]
    let fixture = try #require(try Self.fixture("bridge/cmd_load_drawing.json")["result"] as? [String: Any])
    #expect(Self.keys(try #require(result)) == Self.keys(fixture))
    #expect((result?["loaded"] as? [[String: Any]])?.first?["strokes"] as? Int == 1)
    #expect((result?["unreadable"] as? [[String: Any]])?.count == 1)
    #expect(surface.liveCanvases.first?.drawing.strokes.count == 1)
    await Self.settle()
    #expect(events.named("drawingChanged").isEmpty)
    #expect(events.named("error").first?["code"] as? String == "drawing_unreadable")
    _ = window
  }

  /// Keep both: another device's drawing goes under the person's, and the
  /// two are sent at once as that layer's next revision.
  @Test func keepBothPutsTheirsUnderMineAndSendsBoth() async throws {
    let (surface, events, window) = await Self.surface(try Self.arguments("bridge/create_blank.json"))
    let canvas = try #require(surface.liveCanvases.first)
    // The person's stroke, sent and not saved (another device saved first).
    canvas.drawing = PKDrawing(strokes: [Self.circle(at: CGPoint(x: 150, y: 150))])
    await Self.settle()
    #expect(events.named("drawingChanged").count == 1)
    let theirs = PKDrawing(strokes: [
      Self.circle(at: CGPoint(x: 40, y: 40)), Self.circle(at: CGPoint(x: 80, y: 40)),
    ])
    let data = theirs.dataRepresentation()
    let result = await events.call(
      "loadDrawing",
      [
        "layers": [
          [
            "page_1based": NSNull(), "ink_layer_id": "layer-7", "revision": 3, "keep_mine": true,
            "drawing": [
              "format": "pencilkit", "content_version": 1,
              "bytes_base64": data.base64EncodedString(), "sha256": InkHash.sha256(data),
            ],
          ]
        ]
      ]) as? [String: Any]
    #expect((result?["loaded"] as? [[String: Any]])?.first?["strokes"] as? Int == 3)
    // Theirs first, under the person's.
    #expect(canvas.drawing.strokes.count == 3)
    #expect(canvas.drawing.strokes.last?.renderBounds.midX ?? 0 > 120)
    await Self.settle()
    let both = try #require(events.named("drawingChanged").last)
    #expect(events.named("drawingChanged").count == 2)
    #expect(both["cause"] as? String == "keep_both")
    #expect(both["ink_layer_id"] as? String == "layer-7")
    #expect(both["base_revision"] as? Int == 3)
    #expect(both["stroke_count"] as? Int == 3)
    // What's new on their drawing is the person's stroke.
    let added = try #require(InkRegion.rect(both["new_bounds"]))
    #expect(added.minX > 0.1)
    _ = window
  }

  /// A layer deleted elsewhere: the page shows nothing and has no layer,
  /// and that's no change of the person's.
  @Test func aClearedPageShowsNothingAndIsNoChange() async throws {
    let (surface, events, window) = await Self.surface(try Self.arguments("bridge/create_blank.json"))
    let drawing = PKDrawing(strokes: [Self.circle(at: CGPoint(x: 50, y: 50))])
    let data = drawing.dataRepresentation()
    _ = await events.call(
      "loadDrawing",
      [
        "layers": [
          [
            "page_1based": NSNull(), "ink_layer_id": "layer-3", "revision": 4,
            "drawing": [
              "format": "pencilkit", "content_version": 1,
              "bytes_base64": data.base64EncodedString(), "sha256": InkHash.sha256(data),
            ],
          ]
        ]
      ])
    #expect(surface.liveCanvases.first?.drawing.strokes.count == 1)
    let result = await events.call(
      "loadDrawing",
      ["layers": [["page_1based": NSNull(), "ink_layer_id": NSNull(), "revision": NSNull(), "drawing": NSNull(), "clear": true]]]
    ) as? [String: Any]
    #expect((result?["loaded"] as? [[String: Any]])?.first?["strokes"] as? Int == 0)
    #expect(surface.liveCanvases.first?.drawing.strokes.isEmpty == true)
    #expect(surface.layers[0]?.inkLayerId == nil)
    await Self.settle()
    #expect(events.named("drawingChanged").isEmpty)
    #expect(events.named("error").isEmpty)
    _ = window
  }

  @Test func aStrokeCutShortByAResizeIsReported() async throws {
    let (surface, events, window) = await Self.surface(try Self.arguments("bridge/create_blank.json"))
    let canvas = try #require(surface.liveCanvases.first)
    surface.canvasViewDidBeginUsingTool(canvas)
    surface.view().frame.size.width -= 200
    surface.view().layoutIfNeeded()
    surface.canvasViewDidEndUsingTool(canvas)
    let names = events.sent.map(\.0).filter { $0.hasPrefix("stroke") }
    #expect(names == ["strokeActive", "strokeActive", "strokeInterrupted"])
    let interrupted = try #require(events.named("strokeInterrupted").first)
    #expect(Self.keys(interrupted) == Self.keys(try Self.arguments("bridge/ev_stroke_interrupted.json")))
    #expect(interrupted["reason"] as? String == "resize")
    #expect(interrupted["kept"] as? Bool == false)
    #expect(events.named("strokeActive").map { $0["active"] as? Bool } == [true, false])
    _ = window
  }

  @Test func toolsAndPolicyChangeLive() async throws {
    let (surface, events, window) = await Self.surface(try Self.arguments("bridge/create_blank.json"))
    let fixture = try Self.fixture("bridge/cmd_set_tool.json")
    let result = await events.call("setTool", fixture["arguments"]) as? [String: Any]
    #expect(result?["tool"] as? String == "pen")
    #expect(result?["color"] as? String == "#1A1A1AFF")
    let ink = try #require(surface.liveCanvases.first?.tool as? PKInkingTool)
    #expect(ink.inkType == .pen)
    _ = await events.call("setTool", ["tool": "eraser", "width": 10.0])
    #expect(surface.liveCanvases.first?.tool is PKEraserTool)
    let policy = await events.call("setDrawingPolicy", ["drawing_policy": "pencil_only"]) as? [String: Any]
    #expect(policy?["drawing_policy"] as? String == "pencil_only")
    #expect(surface.liveCanvases.first?.drawingPolicy == PKCanvasViewDrawingPolicy.pencilOnly)
    let undo = await events.call("undo") as? [String: Any]
    #expect(Self.keys(try #require(undo)) == ["can_undo", "can_redo"])
    _ = window
  }

  // MARK: Handwriting (phase 4/6)

  /// Block capitals as a hand prints them, one stroke per pen-down, in a
  /// 50 × 80 box with y down (CoreText's glyph outlines aren't read as
  /// writing: they're contours, not pen paths).
  static let letters: [Character: [[CGPoint]]] = {
    func p(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: x, y: y) }
    let oval = (0...24).map { i -> CGPoint in
      let a = -CGFloat.pi / 2 - CGFloat(i) / 24 * 2 * .pi
      return p(25 + 25 * cos(a), 40 + 40 * sin(a))
    }
    return [
      "H": [[p(0, 0), p(0, 80)], [p(50, 0), p(50, 80)], [p(0, 40), p(50, 40)]],
      "E": [[p(45, 0), p(0, 0), p(0, 80), p(45, 80)], [p(0, 40), p(35, 40)]],
      "L": [[p(0, 0), p(0, 80), p(45, 80)]],
      "O": [oval],
      "I": [[p(25, 0), p(25, 80)]],
      "T": [[p(0, 0), p(50, 0)], [p(25, 0), p(25, 80)]],
      "N": [[p(0, 80), p(0, 0), p(50, 80), p(50, 0)]],
    ]
  }()

  /// Text as handwriting, from `origin` (its top left), at about 12 strokes
  /// a second.
  static func handwritten(_ text: String, at origin: CGPoint, written: Date = Date()) -> [PKStroke] {
    var strokes: [PKStroke] = []
    var date = written
    for (n, letter) in text.enumerated() {
      for path in letters[letter] ?? [] {
        var dense: [CGPoint] = []
        for (a, b) in zip(path, path.dropFirst()) {
          for i in 0..<8 {
            let f = CGFloat(i) / 8
            dense.append(CGPoint(x: a.x + (b.x - a.x) * f, y: a.y + (b.y - a.y) * f))
          }
        }
        dense.append(path[path.count - 1])
        let points = dense.enumerated().map { i, q in
          PKStrokePoint(
            location: CGPoint(x: origin.x + CGFloat(n) * 70 + q.x, y: origin.y + q.y),
            timeOffset: Double(i) * 0.012, size: CGSize(width: 2.5, height: 2.5), opacity: 1,
            force: 1, azimuth: 0, altitude: .pi / 2)
        }
        date += 0.08 + Double(points.count) * 0.012
        strokes.append(
          PKStroke(
            ink: PKInk(.pen, color: .black), path: PKStrokePath(controlPoints: points, creationDate: date)))
      }
    }
    return strokes
  }

  @Test func readySaysWhetherHandwritingCanBeRead() async throws {
    let (_, events, window) = await Self.surface(try Self.arguments("bridge/create_blank.json"))
    let ready = try #require(events.named("ready").first)
    let handwriting = try #require(ready["handwriting"] as? [String: Any])
    if #available(iOS 27.0, *) {
      let fixture = try Self.arguments("bridge/ev_ready_handwriting.json")
      #expect(Self.keys(ready) == Self.keys(fixture))
      #expect(Self.keys(handwriting) == Self.keys(fixture["handwriting"] as! [String: Any]))
      #expect(handwriting["available"] as? Bool == true)
      #expect((handwriting["supported_languages"] as? [String])?.contains("en") == true, "\(handwriting)")
    } else {
      let fixture = try Self.arguments("bridge/ev_ready_handwriting_unavailable.json")
      #expect(Self.keys(handwriting) == Self.keys(fixture["handwriting"] as! [String: Any]))
      #expect(handwriting["available"] as? Bool == false)
    }
    _ = window
  }

  @Test func belowIPadOS27HandwritingIsUnavailable() async throws {
    guard #unavailable(iOS 27.0) else { return }  // runs on the 18.3 simulator
    let (surface, events, window) = await Self.surface(try Self.arguments("bridge/create_blank.json"))
    try #require(surface.liveCanvases.first).drawing = PKDrawing(strokes: Self.handwritten("HELLO", at: CGPoint(x: 100, y: 300)))
    let answer = try #require(await events.call("recognizeText", ["page_1based": NSNull(), "scope": "all"]) as? [String: Any])
    let fixture = try #require(try Self.fixture("bridge/cmd_recognize_text_unavailable.json")["result"] as? [String: Any])
    #expect(Self.keys(answer) == Self.keys(fixture))
    #expect(answer["available"] as? Bool == false)
    #expect(answer["reason"] as? String == "os_too_old")
    #expect(answer["text"] is NSNull)
    _ = window
  }

  @available(iOS 27.0, *)
  @Test func handwritingIsReadOnTheDeviceAndNamesItsDrawing() async throws {
    let (surface, events, window) = await Self.surface(try Self.arguments("bridge/create_blank.json"))
    let canvas = try #require(surface.liveCanvases.first)
    let hello = Self.handwritten("HELLO", at: CGPoint(x: 100, y: 200))
    // Written after it: the recognizer reads strokes in the order drawn.
    let lion = Self.handwritten("LION", at: CGPoint(x: 100, y: 500), written: Date() + 10)
    canvas.drawing = PKDrawing(strokes: hello)
    // Not yet sent (the debounce): reading sends it first.
    let answer = try #require(
      await events.call("recognizeText", ["page_1based": NSNull(), "scope": "all", "languages": ["en-US"]])
        as? [String: Any])
    let fixture = try #require(try Self.fixture("bridge/cmd_recognize_text.json")["result"] as? [String: Any])
    #expect(Self.keys(answer) == Self.keys(fixture))
    let change = try #require(events.named("drawingChanged").last)
    #expect(answer["local_seq"] as? Int == change["local_seq"] as? Int)
    #expect(answer["drawing_sha256"] as? String == (change["drawing"] as? [String: Any])?["sha256"] as? String)
    #expect(answer["stale"] as? Bool == false)
    #expect(answer["stroke_count"] as? Int == hello.count)
    #expect(answer["languages"] as? [String] == ["en-US"])
    #expect(answer["recognition_version"] as? Int == PKStrokeRecognizer.recognitionVersion)
    #expect((answer["text"] as? String)?.lowercased() == "hello", "\(answer)")

    // A second line (synthetic lines read together are unreliable in the
    // Simulator, so they're read apart, as an item or a lasso would).
    canvas.drawing = PKDrawing(strokes: hello + lion)

    // A review item's region: the strokes it meets, only.
    let region = try #require(
      await events.call(
        "recognizeText",
        ["page_1based": NSNull(), "scope": "region", "region": ["x": 0.05, "y": 0.6, "w": 0.5, "h": 0.25]])
        as? [String: Any])
    #expect(region["stroke_count"] as? Int == lion.count)
    #expect((region["text"] as? String)?.lowercased() == "lion", "\(region)")

    // The lasso's selection, as Dart hears of it.
    #expect(
      await events.call("recognizeText", ["page_1based": NSNull(), "scope": "selection"])
        .flatMap { $0 as? [String: Any] }?["text"] is NSNull)
    canvas.selection = Set(hello.map(\.id))
    // PencilKit tells its delegate of the person's lasso, not of code's.
    surface.canvasViewSelectionDidChange(canvas)
    let chosen = try #require(events.named("strokeSelectionChanged").last)
    #expect(Self.keys(chosen) == Self.keys(try Self.arguments("bridge/ev_stroke_selection_changed.json")))
    #expect(chosen["stroke_count"] as? Int == hello.count)
    let selection = try #require(
      await events.call("recognizeText", ["page_1based": NSNull(), "scope": "selection"]) as? [String: Any])
    #expect(selection["stroke_count"] as? Int == hello.count)
    #expect((selection["text"] as? String)?.lowercased() == "hello", "\(selection)")
    // Nothing near: nothing to read.
    let none = try #require(
      await events.call(
        "recognizeText",
        ["page_1based": NSNull(), "scope": "region", "region": ["x": 0.9, "y": 0.0, "w": 0.1, "h": 0.1]])
        as? [String: Any])
    #expect(none["stroke_count"] as? Int == 0)
    #expect(none["text"] is NSNull)
    let bad = await events.call("recognizeText", ["page_1based": NSNull(), "scope": "region"])
    #expect((bad as? FlutterError)?.code == "bad_request")
    _ = window
  }

  @available(iOS 27.0, *)
  @Test func aRecognitionOvertakenByAStrokeIsStale() async throws {
    let (surface, events, window) = await Self.surface(try Self.arguments("bridge/create_blank.json"))
    let canvas = try #require(surface.liveCanvases.first)
    canvas.drawing = PKDrawing(strokes: Self.handwritten("HELLO", at: CGPoint(x: 100, y: 300)))
    let answer: [String: Any] = try #require(
      await withCheckedContinuation { done in
        events.handler!(
          FlutterMethodCall(methodName: "recognizeText", arguments: ["page_1based": NSNull(), "scope": "all"])
        ) { done.resume(returning: $0 as? [String: Any]) }
        // Before the recognizer answers (it can't: this is the main actor):
        // the person draws on.
        canvas.drawing = PKDrawing(strokes: canvas.drawing.strokes + [Self.circle(at: CGPoint(x: 800, y: 600))])
      })
    let fixture = try #require(try Self.fixture("bridge/cmd_recognize_text_stale.json")["result"] as? [String: Any])
    #expect(Self.keys(answer) == Self.keys(fixture))
    #expect(answer["stale"] as? Bool == true)
    #expect(answer["text"] is NSNull)
    _ = window
  }

  // MARK: Interruptions (phase 4/6)

  @Test func leavingTheScreenMidStrokeEndsItAndSendsTheLastChange() async throws {
    for note in [UIApplication.didEnterBackgroundNotification, UIApplication.protectedDataWillBecomeUnavailableNotification] {
      let (surface, events, window) = await Self.surface(try Self.arguments("bridge/create_blank.json"))
      let canvas = try #require(surface.liveCanvases.first)
      // A stroke finished a moment ago, not yet sent (the debounce), and
      // the next one under way.
      canvas.drawing = PKDrawing(strokes: [Self.circle(at: CGPoint(x: 100, y: 100))])
      surface.canvasViewDidBeginUsingTool(canvas)
      NotificationCenter.default.post(name: note, object: nil)
      // At once, not after the debounce.
      let names = events.sent.map(\.0).filter { $0.hasPrefix("stroke") || $0 == "drawingChanged" }
      #expect(names == ["strokeActive", "strokeActive", "strokeInterrupted", "drawingChanged"], "\(note.rawValue)")
      let interrupted = try #require(events.named("strokeInterrupted").first)
      let fixture = try Self.arguments("bridge/ev_stroke_interrupted_background.json")
      #expect(Self.keys(interrupted) == Self.keys(fixture))
      #expect(interrupted["reason"] as? String == "background")
      #expect(interrupted["kept"] as? Bool == false)
      #expect(events.named("drawingChanged").first?["stroke_count"] as? Int == 1)
      // The canvas still draws once the app is back.
      #expect(canvas.drawingGestureRecognizer.isEnabled)
      surface.canvasViewDidEndUsingTool(canvas)
      _ = await events.call("layerSaved", ["page_1based": NSNull(), "ink_layer_id": "layer-1", "revision": 1])
      await Self.settle()
      #expect(events.named("strokeInterrupted").count == 1)
      #expect(events.named("drawingChanged").count == 1)
      _ = window
    }
  }

  @Test func aMemoryWarningDropsCachesNeverInk() async throws {
    let (surface, events, window) = await Self.surface(try Self.arguments("bridge/create_blank.json"))
    let canvas = try #require(surface.liveCanvases.first)
    canvas.drawing = PKDrawing(strokes: Self.handwritten("HI", at: CGPoint(x: 100, y: 300)))
    if #available(iOS 27.0, *) {
      _ = await events.call("recognizeText", ["page_1based": NSNull(), "scope": "all"])
      #expect(surface.recognizers.count == 1)
    }
    canvas.drawing = PKDrawing(strokes: canvas.drawing.strokes + [Self.circle(at: CGPoint(x: 800, y: 600))])
    let strokes = canvas.drawing.strokes.count
    NotificationCenter.default.post(name: UIApplication.didReceiveMemoryWarningNotification, object: nil)
    #expect(surface.recognizers.isEmpty)
    // The pending change went at once; the ink is all there.
    #expect(events.named("drawingChanged").last?["stroke_count"] as? Int == strokes)
    #expect(surface.layers[0]?.drawing.strokes.count == strokes)
    #expect(canvas.drawing.strokes.count == strokes)
    _ = window
  }

  @available(iOS 16.0, *)
  @Test func aStrokeKeptThroughARotationStaysInPageSpace() async throws {
    let (paper, events, window) = try await Self.openPaper()
    let page = try #require(paper.pdf.document?.page(at: 2))
    paper.pdf.go(to: page)
    await Self.settle(0.3)
    let canvas = try #require(paper.liveCanvases.first { $0.tag == 3 })
    paper.canvasViewDidBeginUsingTool(canvas)
    // PencilKit committed the stroke just before the window turned.
    canvas.drawing = PKDrawing(strokes: [Self.circle(at: CGPoint(x: 306, y: 396), radius: 30)])
    window.frame = CGRect(x: 0, y: 0, width: 1000, height: 800)
    paper.view().frame = window.bounds
    window.layoutIfNeeded()
    let interrupted = try #require(events.named("strokeInterrupted").first)
    #expect(interrupted["reason"] as? String == "resize")
    #expect(interrupted["kept"] as? Bool == true)
    #expect(interrupted["page_1based"] as? Int == 3)
    await Self.settle()
    // Sent once the stroke ended, in the page's own space: the middle of
    // page 3, whatever the new layout's zoom.
    let change = try #require(events.named("drawingChanged").last)
    #expect(change["page_1based"] as? Int == 3)
    let added = try #require(InkRegion.rect(change["new_bounds"]))
    #expect(abs(added.midX - 0.5) < 0.01 && abs(added.midY - 0.5) < 0.01)
    // A stroke not yet committed when the layout changes is dropped.
    paper.canvasViewDidBeginUsingTool(canvas)
    window.frame = CGRect(x: 0, y: 0, width: 800, height: 1000)
    paper.view().frame = window.bounds
    window.layoutIfNeeded()
    #expect(events.named("strokeInterrupted").last?["kept"] as? Bool == false)
    #expect(canvas.drawing.strokes.count == 1)
    _ = window
  }

  // MARK: A PDF changing while open

  @available(iOS 16.0, *)
  @Test func aNewVersionShowsInPlaceAndOutdatesOnlyTheInkOfChangedPages() async throws {
    // Its creation params name ink on pages 3, 5 and 7 (create_pdf.json).
    let (paper, events, window) = try await Self.openPaper()
    let ink = PKDrawing(strokes: [Self.circle(at: CGPoint(x: 306, y: 396))]).dataRepresentation()
    func layer(_ page: Int) -> [String: Any] {
      [
        "page_1based": page, "ink_layer_id": "layer-p\(page)", "revision": 1,
        "drawing": [
          "format": "pencilkit", "content_version": 1, "bytes_base64": ink.base64EncodedString(),
          "sha256": InkHash.sha256(ink),
        ],
      ]
    }
    _ = await events.call("loadDrawing", ["layers": [layer(3), layer(5)]])
    let three = try #require(paper.pdf.document?.page(at: 2))
    paper.pdf.autoScales = false
    paper.pdf.scaleFactor = 2
    paper.pdf.go(to: PDFDestination(page: three, at: CGPoint(x: 100, y: 500)))
    await Self.settle(0.3)
    let canvas = try #require(paper.liveCanvases.first { $0.tag == 3 })
    paper.canvasViewDidBeginUsingTool(canvas)
    let doc = try #require(paper.pdf.document)
    let wasPage = doc.index(for: try #require(paper.pdf.currentPage)) + 1
    let was = try #require(PaperView.spot(in: paper.pdf))
    // Ink drawn on page 2 and not saved yet: still shown, so kept too.
    paper.layers[2] = InkSurface.Layer(
      drawing: PKDrawing(strokes: [Self.circle(at: CGPoint(x: 100, y: 100))]))

    // Version 2: page 5's sentence changed, a page 9 added.
    let (file, sha) = try Self.paper(pages: 9, edited: [5])
    let fixture = try Self.fixture("bridge/cmd_reload_document.json")
    let answer = try #require(
      await events.call("reloadDocument", ["source": ["sha256": sha], "document": ["file": file.path, "sha256": sha]])
        as? [String: Any])
    #expect(Self.keys(answer) == Self.keys(fixture["result"] as! [String: Any]))
    #expect(answer["reloaded"] as? Bool == true)
    #expect(answer["source_sha256"] as? String == sha)
    #expect(answer["page_count"] as? Int == 9)
    #expect(answer["pages_changed"] as? [Int] == [5])
    #expect(answer["pages_added"] as? [Int] == [9])
    #expect(answer["pages_removed"] as? [Int] == [])
    let kept = answer["kept_layers"] as? [[String: Any]] ?? []
    #expect(kept.map { $0["page_1based"] as? Int } == [2, 3, 7])
    #expect(kept.map { $0["ink_layer_id"] as? String } == [nil, "layer-p3", "layer-paper-p7"])
    #expect(kept.first?["ink_layer_id"] is NSNull && kept.first?["revision"] is NSNull)
    #expect((answer["outdated_layers"] as? [[String: Any]])?.map { $0["ink_layer_id"] as? String } == ["layer-p5"])
    // Where the person was, at their zoom (which page PDFKit calls
    // current at that spot differs between iPadOS 18 and 27).
    #expect(answer["page_1based"] as? Int == wasPage)
    let now = try #require(PaperView.spot(in: paper.pdf))
    #expect(now.index == was.index)
    #expect(abs(now.point.x - was.point.x) < 2 && abs(now.point.y - was.point.y) < 2, "\(was) → \(now)")
    #expect(abs(paper.pdf.scaleFactor - 2) < 0.01)
    // The stroke under way ended with the old version.
    let cut = try #require(events.named("strokeInterrupted").last)
    #expect(cut["reason"] as? String == "reload")
    #expect(Self.keys(cut) == Self.keys(try Self.arguments("bridge/ev_stroke_interrupted_reload.json")))
    // Page 3's ink is shown on the new version; page 5's isn't moved onto it.
    await Self.settle(0.3)
    #expect(paper.liveCanvases.first { $0.tag == 3 }?.drawing.strokes.count == 1)
    #expect(paper.layers[5] == nil)
    paper.pdf.go(to: try #require(paper.pdf.document?.page(at: 4)))
    await Self.settle(0.3)
    #expect(paper.liveCanvases.first { $0.tag == 5 }?.drawing.strokes.isEmpty == true)
    // What's drawn next is on the new version.
    paper.pdf.go(to: try #require(paper.pdf.document?.page(at: 2)))
    await Self.settle(0.3)
    let page3 = try #require(paper.liveCanvases.first { $0.tag == 3 })
    page3.drawing = PKDrawing(strokes: page3.drawing.strokes + [Self.circle(at: CGPoint(x: 100, y: 100))])
    await Self.settle()
    #expect(events.named("drawingChanged").last?["source_sha256"] as? String == sha)

    // A version that isn't the one named leaves this one shown.
    let (other, _) = try Self.paper(pages: 4)
    let refused = await events.call(
      "reloadDocument", ["source": ["sha256": sha], "document": ["file": other.path, "sha256": sha]])
    let error = try #require(refused as? FlutterError)
    #expect(error.code == "document_hash_mismatch")
    #expect((error.details as? [String: Any])?["shown_sha256"] as? String == sha)
    #expect(paper.pdf.document?.pageCount == 9)

    // Fewer pages: those gone are removed, their ink outdated.
    let (short, shortSha) = try Self.paper(pages: 2)
    let shorter = try #require(
      await events.call(
        "reloadDocument", ["source": ["sha256": shortSha], "document": ["file": short.path, "sha256": shortSha]])
        as? [String: Any])
    #expect(shorter["pages_removed"] as? [Int] == Array(3...9))
    #expect((shorter["outdated_layers"] as? [[String: Any]])?.map { $0["page_1based"] as? Int } == [3, 7])
    #expect(shorter["page_1based"] as? Int == 1)
    _ = window
  }

  /// A long paper: every page with its number and a few lines; `edited`
  /// pages say something else.
  static func long(pages: Int, edited: Set<Int> = []) throws -> (URL, String) {
    let bounds = CGRect(x: 0, y: 0, width: 612, height: 792)
    let font = UIFont.systemFont(ofSize: 11)
    let data = UIGraphicsPDFRenderer(bounds: bounds).pdfData { context in
      for n in 1...pages {
        context.beginPage()
        ("Page \(n)" as NSString).draw(at: CGPoint(x: 72, y: 72), withAttributes: [.font: font])
        for line in 0..<20 {
          let text = edited.contains(n) && line == 7
            ? "This line was rewritten in the new version."
            : "Line \(line) of page \(n): the hazard is estimated by a smoothed baseline."
          (text as NSString).draw(at: CGPoint(x: 72, y: 100 + CGFloat(line) * 16), withAttributes: [.font: font])
        }
      }
    }
    let url = FileManager.default.temporaryDirectory.appendingPathComponent("ink-\(UUID()).pdf")
    try data.write(to: url)
    return (url, try #require(InkHash.sha256(file: url)))
  }

  @available(iOS 16.0, *)
  @Test func comparing2000PagesIsQuick() async throws {
    let (a, _) = try Self.long(pages: 2000)
    let (b, _) = try Self.long(pages: 2000, edited: [1000, 1500])
    let before = try #require(PDFDocument(url: a)), after = try #require(PDFDocument(url: b))
    // Off the main thread, as a reload does; ink on pages 3 and 1000.
    let (changed, seconds) = await Task.detached {
      let start = Date()
      let changed = PaperView.changedPages(from: before, to: after, inked: [3, 1000])
      return (changed, Date().timeIntervalSince(start))
    }.value
    #expect(changed == [1000, 1500])
    // The closer look alone (text and a rendering), page by page, for scale.
    let (slow, slowSeconds) = await Task.detached {
      let start = Date()
      var same = 0
      for i in 0..<100 where PaperView.looksTheSame(before.page(at: i)!, after.page(at: i)!) { same += 1 }
      return (same, Date().timeIntervalSince(start))
    }.value
    #expect(slow == 100)
    // Measured on the 27.2 Simulator under load: 0.49 s by makings; the
    // closer look alone would take about 6.9 s for 2000 pages.
    _ = slowSeconds
    #expect(seconds < 2, "\(seconds) s")
  }

  // MARK: Pencil (double-tap, squeeze, hover)

  @available(iOS 16.0, *)
  @Test func pencilGesturesChangeToolsAndShowPalettesNeverMore() async throws {
    let (surface, events, window) = await Self.surface(try Self.arguments("bridge/create_blank.json"))
    let canvas = try #require(surface.liveCanvases.first)
    canvas.drawing = PKDrawing(strokes: [Self.circle(at: CGPoint(x: 100, y: 100))])
    await Self.settle()
    let sentBefore = events.named("drawingChanged").count
    _ = await events.call("setTool", ["tool": "pen", "color": "#1A1A1AFF", "width": 3.0])

    surface.perform(.switchEraser, source: "pencil_double_tap", at: nil)
    #expect(canvas.tool is PKEraserTool)
    #expect(events.named("toolChanged").last?["tool"] as? String == "eraser")
    #expect(events.named("toolChanged").last?["source"] as? String == "pencil_double_tap")
    // Again: back to the pen, by a squeeze this time.
    surface.perform(.switchEraser, source: "pencil_squeeze", at: nil)
    let back = try #require(events.named("toolChanged").last)
    #expect(Self.keys(back) == Self.keys(try Self.arguments("bridge/ev_tool_changed_squeeze.json")))
    #expect(back["tool"] as? String == "pen" && back["source"] as? String == "pencil_squeeze")
    #expect((canvas.tool as? PKInkingTool)?.inkType == .pen)
    surface.perform(.switchPrevious, source: "pencil_double_tap", at: nil)
    #expect(canvas.tool is PKEraserTool)

    // A palette is Dart's to show, where the Pencil is.
    surface.perform(.showColorPalette, source: "pencil_squeeze", at: CGPoint(x: 400, y: 250))
    let palette = try #require(events.named("pencilAction").last)
    #expect(Self.keys(palette) == Self.keys(try Self.arguments("bridge/ev_pencil_action.json")))
    #expect(palette["action"] as? String == "show_color_palette")
    #expect(palette["at"] as? [String: Double] == ["x": 0.5, "y": 0.25])
    surface.perform(.showInkAttributes, source: "pencil_double_tap", at: nil)
    #expect(events.named("pencilAction").last?["action"] as? String == "show_ink_attributes")
    #expect(events.named("pencilAction").last?["at"] is NSNull)
    // Ignore does nothing.
    let count = events.sent.count
    surface.perform(.ignore, source: "pencil_double_tap", at: nil)
    #expect(events.sent.count == count)
    // The delegate follows what's chosen in Settings.
    surface.pencilInteractionDidTap(UIPencilInteraction())
    if UIPencilInteraction.preferredTapAction == .ignore { #expect(events.sent.count == count) }

    // Never anything else: no ink changed, nothing to undo.
    await Self.settle()
    #expect(events.named("drawingChanged").count == sentBefore)
    #expect(canvas.drawing.strokes.count == 1)
    #expect(Set(events.sent.map(\.0)).isSubset(of: ["drawingChanged", "undoState", "toolChanged", "pencilAction", "ready", "strokeActive"]))
    _ = window
  }

  @available(iOS 16.0, *)
  @Test func hoverPreviewsTheMarkAtThePagesZoom() async throws {
    let (paper, events, window) = try await Self.openPaper()
    paper.pdf.autoScales = false
    paper.pdf.scaleFactor = 2
    _ = await events.call("setTool", ["tool": "marker", "color": "#D03030FF", "width": 6.0])
    let at = CGPoint(x: 300, y: 400)
    paper.showHover(at: at)
    #expect(!paper.hoverLayer.isHidden)
    let box = try #require(paper.hoverLayer.path?.boundingBox)
    #expect(abs(box.width - 12) < 0.01 && abs(box.midX - at.x) < 0.01 && abs(box.midY - at.y) < 0.01)
    let fill = try #require(paper.hoverLayer.fillColor.map(UIColor.init(cgColor:)))
    #expect(fill.inkHex == "#D0303080")
    // An eraser shows a ring; a lasso, nothing; nothing while drawing is
    // off or a stroke is under way.
    _ = await events.call("setTool", ["tool": "eraser", "width": 20.0])
    paper.showHover(at: at)
    #expect(paper.hoverLayer.fillColor == nil && paper.hoverLayer.strokeColor != nil)
    #expect(abs((paper.hoverLayer.path?.boundingBox.width ?? 0) - 40) < 0.01)
    _ = await events.call("setTool", ["tool": "lasso"])
    paper.showHover(at: at)
    #expect(paper.hoverLayer.isHidden)
    _ = await events.call("setTool", ["tool": "pen", "color": "#000000FF", "width": 1.0])
    paper.showHover(at: at)
    #expect(!paper.hoverLayer.isHidden)
    // Never smaller than a few points.
    #expect(abs((paper.hoverLayer.path?.boundingBox.width ?? 0) - 4) < 0.01)
    let canvas = try #require(paper.liveCanvases.first)
    paper.canvasViewDidBeginUsingTool(canvas)
    #expect(paper.hoverLayer.isHidden)
    paper.canvasViewDidEndUsingTool(canvas)
    _ = await events.call("setDrawingPolicy", ["drawing_policy": "none"])
    paper.showHover(at: at)
    #expect(paper.hoverLayer.isHidden)
    // Only a preview: nothing drawn.
    await Self.settle()
    #expect(events.named("drawingChanged").isEmpty)
    _ = window
  }

  // MARK: Ink over a dark theme

  /// Red, green, blue (0…1) of a pixel of an image, from its top left.
  static func pixel(_ image: CGImage, _ x: Int, _ y: Int) -> (Double, Double, Double) {
    var rgba = [UInt8](repeating: 0, count: 4)
    let context = CGContext(
      data: &rgba, width: 1, height: 1, bitsPerComponent: 8, bytesPerRow: 4,
      space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.draw(image, in: CGRect(x: -x, y: y - image.height + 1, width: image.width, height: image.height))
    return (Double(rgba[0]) / 255, Double(rgba[1]) / 255, Double(rgba[2]) / 255)
  }

  @Test func inkKeepsItsColoursInDarkMode() async throws {
    let (surface, events, window) = await Self.surface(try Self.arguments("bridge/create_blank.json"))
    window.overrideUserInterfaceStyle = .dark
    window.layoutIfNeeded()
    let canvas = try #require(surface.liveCanvases.first)
    #expect(canvas.traitCollection.userInterfaceStyle == .light)
    // A thick black line across the sketch's middle.
    let points = (0...30).map { i in
      PKStrokePoint(
        location: CGPoint(x: 200 + CGFloat(i) * 20, y: 384), timeOffset: Double(i) * 0.01,
        size: CGSize(width: 24, height: 24), opacity: 1, force: 1, azimuth: 0, altitude: .pi / 2)
    }
    canvas.drawing = PKDrawing(strokes: [
      PKStroke(ink: PKInk(.pen, color: .black), path: PKStrokePath(controlPoints: points, creationDate: Date()))
    ])
    await Self.settle(0.6)
    // On screen.
    let view = surface.view()
    let shown = UIGraphicsImageRenderer(bounds: view.bounds).image { _ in
      view.drawHierarchy(in: view.bounds, afterScreenUpdates: true)
    }
    let cg = try #require(shown.cgImage)
    let scale = shown.scale
    // The sketch (1024 × 768) fits the 800 × 1000 view: 800/1024 wide.
    let fit = view.bounds.width / 1024
    let lineY = Int((view.bounds.height - 768 * fit) / 2 * scale + 384 * fit * scale)
    let onInk = Self.pixel(cg, Int(500 * fit * scale), lineY)
    let onPaper = Self.pixel(cg, Int(500 * fit * scale), lineY - Int(80 * fit * scale))
    #expect(onInk.0 < 0.25 && onInk.1 < 0.25 && onInk.2 < 0.25, "\(onInk)")
    #expect(onPaper.0 > 0.9 && onPaper.1 > 0.9 && onPaper.2 > 0.9, "\(onPaper)")
    // In the exported picture too.
    let out = try #require(
      await events.call("exportComposite", ["region": ["x": 0.0, "y": 0.0, "w": 1.0, "h": 1.0], "include_ink": true])
        as? [String: Any])
    let file = try #require(out["file"] as? String)
    let picture = try #require(UIImage(contentsOfFile: file)?.cgImage)
    let k = Double(picture.width) / 1024
    let ink = Self.pixel(picture, Int(500 * k), Int(384 * k))
    let paper = Self.pixel(picture, Int(500 * k), Int(300 * k))
    #expect(ink.0 < 0.25 && ink.1 < 0.25 && ink.2 < 0.25, "\(ink)")
    #expect(paper.0 > 0.9 && paper.1 > 0.9 && paper.2 > 0.9, "\(paper)")
    _ = window
  }

  // MARK: Accessibility

  @available(iOS 16.0, *)
  @Test func voiceOverReadsWhereYouAreAndWhetherYouDrewThere() async throws {
    let (paper, events, window) = try await Self.openPaper()
    func read() -> (String?, String?) {
      let first = paper.view().accessibilityElements?.first as? UIAccessibilityElement
      return (first?.accessibilityLabel, first?.accessibilityValue)
    }
    #expect(read().0 == "Page 1 of 8")
    #expect(read().1 == nil)
    // Then the paper itself, which PDFKit reads.
    #expect(paper.view().accessibilityElements?.last as? PDFView === paper.pdf)
    let frame = try #require(paper.view().accessibilityElements?.first as? UIAccessibilityElement)
      .accessibilityFrameInContainerSpace
    #expect(frame.width == paper.view().bounds.width && frame.height == 44)
    let ink = PKDrawing(strokes: [Self.circle(at: CGPoint(x: 306, y: 396))]).dataRepresentation()
    _ = await events.call(
      "loadDrawing",
      [
        "layers": [
          [
            "page_1based": 3, "ink_layer_id": "layer-paper-p3", "revision": 2,
            "drawing": [
              "format": "pencilkit", "content_version": 1, "bytes_base64": ink.base64EncodedString(),
              "sha256": InkHash.sha256(ink),
            ],
          ]
        ]
      ])
    _ = await events.call("goToPage", ["page_1based": 3])
    await Self.settle(0.3)
    #expect(read().0 == "Page 3 of 8")
    #expect(read().1 == "You drew on this page")
    // Erased: no longer said.
    let canvas = try #require(paper.liveCanvases.first { $0.tag == 3 })
    canvas.drawing = PKDrawing()
    #expect(read().1 == nil)
    _ = window
  }

  @Test func aSketchAndAQuoteSayWhatTheyAre() async throws {
    let (sketch, _, w1) = await Self.surface(try Self.arguments("bridge/create_blank.json"))
    #expect((sketch.view().accessibilityElements?.first as? UIAccessibilityElement)?.accessibilityLabel == "Sketch")
    let (quote, _, w2) = await Self.surface(try Self.arguments("bridge/create_excerpt.json"))
    let elements = try #require(quote.view().accessibilityElements)
    #expect((elements.first as? UIAccessibilityElement)?.accessibilityLabel == "Quote")
    let text = try #require(elements.last as? InkExcerptView)
    #expect(text.isAccessibilityElement)
    #expect(text.accessibilityLabel?.isEmpty == false)
    _ = (w1, w2)
  }

  @available(iOS 16.0, *)
  @Test func nothingNeedsDrawing() async throws {
    // Drawing off: a quote chosen as text, its page region and a picture
    // of it without ink make a review item; search and pages work too.
    let (file, sha) = try Self.paper()
    var args = try Self.pdfArgs(file: file, sha: sha)
    args["drawing_policy"] = "none"
    let (surface, events, window) = await Self.surface(args)
    await Self.settle()
    let paper = try #require(surface as? PaperView)
    let page = try #require(paper.pdf.document?.page(at: 2))
    let found = try #require(paper.pdf.document?.findString("vanishing", withOptions: []).first { $0.pages.first == page })
    paper.pdf.setCurrentSelection(found, animate: false)
    await Self.settle(0.2)
    let chosen = try #require(events.named("selectionChanged").last)
    #expect((chosen["text"] as? String)?.contains("vanishing") == true)
    let region = try #require(InkRegion.rect(chosen["region"]))
    let picture = try #require(
      await events.call(
        "exportComposite", ["page_1based": 3, "region": InkRegion.json(region), "include_ink": false]) as? [String: Any])
    #expect(FileManager.default.fileExists(atPath: try #require(picture["file"] as? String)))
    let search = await events.call("search", ["query": "gradient"]) as? [String: Any]
    #expect((search?["matches"] as? [Any])?.count == 2)
    #expect((await events.call("goToPage", ["page_1based": 5]) as? [String: Any])?["page_1based"] as? Int == 5)
    #expect(paper.liveCanvases.allSatisfy { !$0.draws })
    // Opened with drawing off: never in markup mode.
    #expect(!paper.pdf.isInMarkupMode)
    _ = window
  }

  // MARK: Export

  @available(iOS 16.0, *)
  @Test func aCompositeMapsBackOntoItsRegion() async throws {
    let (file, sha) = try Self.paper()
    let (surface, _, window) = await Self.surface(try Self.pdfArgs(file: file, sha: sha))
    await Self.settle()
    let paper = try #require(surface as? PaperView)
    let region = CGRect(x: 0.08, y: 0.27, width: 0.58, height: 0.26)
    for n in [1, 5, 7] {
      let page = try #require(paper.pdf.document?.page(at: n - 1))
      let request = InkExport.Request(["max_long_edge": 2048, "max_bytes": 3_145_728])
      let out = try #require(InkExport.pdf(page: page, region: region, drawing: PKDrawing(), request: request))
      let w = try #require(out["pixel_width"] as? Int), h = try #require(out["pixel_height"] as? Int)
      #expect(max(w, h) <= 2048)
      #expect((out["bytes"] as? Int ?? .max) <= 3_145_728)
      // Its corners land on the region's corners in PDF user space.
      let t = try #require(out["crop_to_document_transform"] as? [Double])
      func map(_ u: Double, _ v: Double) -> CGPoint {
        CGPoint(x: t[0] * u + t[2] * v + t[4], y: t[1] * u + t[3] * v + t[5])
      }
      let corners = [map(0, 0), map(Double(w), 0), map(0, Double(h)), map(Double(w), Double(h))]
      let crop = page.bounds(for: .cropBox)
      let want = CGRect(
        x: crop.minX + region.minX * crop.width, y: crop.maxY - region.maxY * crop.height,
        width: region.width * crop.width, height: region.height * crop.height)
      let got = CGRect(
        x: corners.map(\.x).min()!, y: corners.map(\.y).min()!,
        width: corners.map(\.x).max()! - corners.map(\.x).min()!,
        height: corners.map(\.y).max()! - corners.map(\.y).min()!)
      #expect(abs(got.minX - want.minX) < 1 && abs(got.minY - want.minY) < 1, "page \(n)")
      #expect(abs(got.maxX - want.maxX) < 1 && abs(got.maxY - want.maxY) < 1, "page \(n)")
      // Shown turned: the page at /Rotate 90 gives a tall image of a wide region.
      if n == 5 { #expect(h > w) } else { #expect(w > h) }
      let png = try Data(contentsOf: URL(fileURLWithPath: out["file"] as! String))
      #expect(InkHash.sha256(png) == out["sha256"] as? String)
    }
    _ = window
  }

  // MARK: Keeping drawings (cmd_export_layer)

  /// The most opaque alpha (0…1) in a small square of an image around a
  /// point, from its top left.
  static func alpha(_ image: CGImage, around x: Int, _ y: Int, radius: Int = 4) -> Double {
    let side = 2 * radius + 1
    var rgba = [UInt8](repeating: 0, count: 4 * side * side)
    let context = CGContext(
      data: &rgba, width: side, height: side, bitsPerComponent: 8, bytesPerRow: 4 * side,
      space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.draw(
      image,
      in: CGRect(
        x: -(x - radius), y: -(image.height - (y + radius + 1)), width: image.width,
        height: image.height))
    return Double(stride(from: 3, to: rgba.count, by: 4).map { rgba[$0] }.max() ?? 0) / 255
  }

  /// An export's two files, checked: the drawing's bytes are what it says
  /// and open as a drawing; the preview is a PNG of the size it says.
  static func files(_ out: [String: Any]) throws -> (PKDrawing, CGImage) {
    let drawing = try #require(out["drawing"] as? [String: Any])
    let preview = try #require(out["ink_preview"] as? [String: Any])
    let data = try Data(contentsOf: URL(fileURLWithPath: try #require(drawing["file"] as? String)))
    #expect(InkHash.sha256(data) == drawing["sha256"] as? String)
    #expect(data.count == drawing["bytes"] as? Int)
    let png = try Data(contentsOf: URL(fileURLWithPath: try #require(preview["file"] as? String)))
    #expect(InkHash.sha256(png) == preview["sha256"] as? String)
    #expect(png.count == preview["bytes"] as? Int)
    #expect(png.count <= 4 * 1024 * 1024)
    for file in [drawing["file"], preview["file"]] {
      let attributes = try FileManager.default.attributesOfItem(atPath: try #require(file as? String))
      #expect((attributes[.posixPermissions] as? NSNumber)?.intValue == 0o600)
    }
    let image = try #require(UIImage(data: png)?.cgImage)
    #expect(image.width == preview["pixel_width"] as? Int && image.height == preview["pixel_height"] as? Int)
    #expect(image.alphaInfo != .none && image.alphaInfo != .noneSkipLast && image.alphaInfo != .noneSkipFirst)
    return (try PKDrawing(data: data), image)
  }

  @available(iOS 16.0, *)
  @Test func aPageExportsAsItsLayerKeepsIt() async throws {
    // Pages as the evidence fixtures' are: plain (3), turned (5), cropped (7).
    let (paper, events, window) = try await Self.openPaper()
    let ink = PKDrawing(strokes: [Self.circle(at: CGPoint(x: 150, y: 200), radius: 40)])
    let data = ink.dataRepresentation()
    _ = await events.call(
      "loadDrawing",
      [
        "layers": [3, 5, 7].map { n -> [String: Any] in
          [
            "page_1based": n, "ink_layer_id": "layer-paper-p\(n)", "revision": n,
            "drawing": [
              "format": "pencilkit", "content_version": 1,
              "bytes_base64": data.base64EncodedString(), "sha256": InkHash.sha256(data),
            ],
          ]
        }
      ])
    let fixture = try Self.fixture("bridge/cmd_export_layer.json")
    let wanted = try #require(fixture["result"] as? [String: Any])
    let sent = events.named("drawingChanged").count
    for (n, evidence) in [
      (3, "layer_pdf_page3.json"), (5, "layer_pdf_page5_rotated.json"), (7, "layer_pdf_page7_cropped.json"),
    ] {
      let layer = try Self.fixture("evidence/\(evidence)")
      var args = try Self.arguments("bridge/cmd_export_layer.json")
      args["page_1based"] = n
      let out = try #require(await events.call("exportLayer", args) as? [String: Any], "page \(n)")
      #expect(Self.keys(out) == Self.keys(wanted), "page \(n)")
      #expect(out["page_1based"] as? Int == n)
      #expect(out["ink_layer_id"] as? String == "layer-paper-p\(n)")
      #expect(out["base_revision"] as? Int == n)
      // Unchanged since loaded: no drawingChanged is this one.
      #expect(out["local_seq"] is NSNull)
      #expect(out["stroke_count"] as? Int == 1)
      #expect(out["source_sha256"] as? String == paper.sourceSha256)
      // Where it lies is what its layer keeps.
      let space = try #require(out["coordinate_space"] as? [String: Any])
      let kept = try #require(layer["coordinate_space"] as? [String: Any])
      #expect(NSDictionary(dictionary: space).isEqual(to: kept), "page \(n): \(space)")
      let drawing = try #require(out["drawing"] as? [String: Any])
      let keptDrawing = try #require(layer["drawing"] as? [String: Any])
      #expect(Self.keys(drawing) == Self.keys(try #require(wanted["drawing"] as? [String: Any])))
      #expect(drawing["space"] as? String == "page_overlay")
      #expect(drawing["drawing_to_document"] as? [Double] == keptDrawing["drawing_to_document"] as? [Double])
      #expect(drawing["content_version"] as? Int == ink.inkContentVersion)
      let (read, preview) = try Self.files(out)
      #expect(read.strokes.count == 1)
      // At 2 pixels a point, turned as the page is shown: the evidence's sizes.
      let keptPreview = try #require(layer["ink_preview"] as? [String: Any])
      #expect(preview.width == keptPreview["pixel_width"] as? Int, "page \(n)")
      #expect(preview.height == keptPreview["pixel_height"] as? Int, "page \(n)")
      // The ink, on nothing: clear in a corner, inked on the circle.
      #expect(Self.alpha(preview, around: 4, 4) == 0, "page \(n)")
      let onRing = CGPoint(x: 150 + 40, y: 200)
      let shown: CGPoint
      switch n {
      // /Rotate 90: the page's top left is shown at the top right.
      case 5: shown = CGPoint(x: CGFloat(preview.width) - onRing.y * 2, y: onRing.x * 2)
      default: shown = CGPoint(x: onRing.x * 2, y: onRing.y * 2)
      }
      #expect(Self.alpha(preview, around: Int(shown.x), Int(shown.y)) > 0.5, "page \(n) at \(shown)")
    }
    // Exporting is no change.
    #expect(events.named("drawingChanged").count == sent)
    _ = window
  }

  @Test func aChangeGoesBeforeItsExportAndNamesIt() async throws {
    // A sketch: 1024 x 768 points, its PNG twice that (layer_blank_sketch).
    let (surface, events, window) = await Self.surface(try Self.arguments("bridge/create_blank.json"))
    await Self.settle()
    let canvas = try #require(surface.liveCanvases.first)
    canvas.drawing = PKDrawing(strokes: [Self.circle(at: CGPoint(x: 100, y: 100))])
    // At once, before the change's own debounce.
    let out = try #require(await events.call("exportLayer", ["page_1based": NSNull()]) as? [String: Any])
    let change = try #require(events.named("drawingChanged").last)
    #expect(out["local_seq"] as? Int == change["local_seq"] as? Int)
    #expect(out["page_1based"] is NSNull && out["ink_layer_id"] is NSNull && out["base_revision"] is NSNull)
    let evidence = try Self.fixture("evidence/layer_blank_sketch.json")
    let space = try #require(out["coordinate_space"] as? [String: Any])
    #expect(NSDictionary(dictionary: space).isEqual(to: try #require(evidence["coordinate_space"] as? [String: Any])))
    let drawing = try #require(out["drawing"] as? [String: Any])
    #expect(drawing["space"] as? String == "blank_canvas")
    #expect(drawing["drawing_to_document"] as? [Double] == [2, 0, 0, 2, 0, 0])
    let (read, preview) = try Self.files(out)
    #expect(read == canvas.drawing)
    #expect((preview.width, preview.height) == (2048, 1536))
    _ = window
  }

  @available(iOS 16.0, *)
  @Test func aPageNeverDrawnOnHasNothingToExportButOneErasedHas() async throws {
    let (paper, events, window) = try await Self.openPaper()
    let none = await events.call("exportLayer", ["page_1based": 2])
    #expect((none as? FlutterError)?.code == "nothing_drawn")
    let page = try #require(paper.pdf.document?.page(at: 1))
    paper.pdf.go(to: page)
    await Self.settle(0.3)
    let canvas = try #require(paper.liveCanvases.first { $0.tag == 2 })
    canvas.drawing = PKDrawing(strokes: [Self.circle(at: CGPoint(x: 100, y: 100))])
    await Self.settle(0.4)
    canvas.drawing = PKDrawing()
    await Self.settle(0.4)
    // Erased: a new, empty revision to save.
    let out = try #require(await events.call("exportLayer", ["page_1based": 2]) as? [String: Any])
    #expect(out["stroke_count"] as? Int == 0)
    #expect(out["local_seq"] as? Int == events.named("drawingChanged").last?["local_seq"] as? Int)
    let (read, preview) = try Self.files(out)
    #expect(read.strokes.isEmpty)
    #expect((preview.width, preview.height) == (1224, 1584))
    _ = window
  }

  @Test func anImageAndAQuoteExportInTheirOwnUnits() async throws {
    // A photo stored 400 x 300 with EXIF orientation 6: shown 300 x 400.
    let one = UIGraphicsImageRendererFormat()
    one.scale = 1
    let stored = UIGraphicsImageRenderer(size: CGSize(width: 400, height: 300), format: one).image { context in
      UIColor.orange.setFill()
      context.fill(CGRect(x: 0, y: 0, width: 400, height: 300))
    }
    let jpeg = NSMutableData()
    let destination = try #require(CGImageDestinationCreateWithData(jpeg, "public.jpeg" as CFString, 1, nil))
    CGImageDestinationAddImage(
      destination, try #require(stored.cgImage),
      [
        kCGImagePropertyOrientation: NSNumber(value: 6),
        kCGImagePropertyTIFFDictionary: [kCGImagePropertyTIFFOrientation: NSNumber(value: 6)],
        kCGImageDestinationLossyCompressionQuality: NSNumber(value: 0.9),
      ] as CFDictionary)
    #expect(CGImageDestinationFinalize(destination))
    let written = CGImageSourceCreateWithData(jpeg, nil).flatMap { CGImageSourceCopyPropertiesAtIndex($0, 0, nil) as? [CFString: Any] }
    #expect((written?[kCGImagePropertyOrientation] as? NSNumber)?.intValue == 6, "the test's photo: \(String(describing: written))")
    let url = FileManager.default.temporaryDirectory.appendingPathComponent("ink-\(UUID()).jpg")
    try (jpeg as Data).write(to: url)
    var args = try Self.arguments("bridge/create_image.json")
    let sha = InkHash.sha256(jpeg as Data)
    args["document"] = ["file": url.path, "sha256": sha]
    var source = try #require(args["source"] as? [String: Any])
    source["sha256"] = sha
    args["source"] = source
    let (image, events, w1) = await Self.surface(args)
    await Self.settle()
    image.liveCanvases.first?.drawing = PKDrawing(strokes: [Self.circle(at: CGPoint(x: 150, y: 200))])
    let out = try #require(await events.call("exportLayer", ["page_1based": NSNull()]) as? [String: Any])
    let space = try #require(out["coordinate_space"] as? [String: Any])
    #expect(Self.keys(space) == Self.keys(try #require(try Self.fixture("evidence/layer_image_exif.json")["coordinate_space"] as? [String: Any])))
    #expect(space["kind"] as? String == "image" && space["units"] as? String == "px")
    #expect((space["oriented_width"] as? Int, space["oriented_height"] as? Int) == (300, 400))
    #expect((space["stored_width"] as? Int, space["stored_height"] as? Int) == (400, 300))
    #expect(space["exif_orientation"] as? Int == 6)
    #expect((out["drawing"] as? [String: Any])?["space"] as? String == "oriented_image")
    let (_, imagePreview) = try Self.files(out)
    #expect((imagePreview.width, imagePreview.height) == (600, 800))

    // A quote: its rendition's points, its text and how it's set.
    let (quote, quoteEvents, w2) = await Self.surface(try Self.arguments("bridge/create_excerpt.json"))
    await Self.settle()
    quote.liveCanvases.first?.drawing = PKDrawing(strokes: [Self.circle(at: CGPoint(x: 100, y: 60))])
    let said = try #require(await quoteEvents.call("exportLayer", ["page_1based": NSNull()]) as? [String: Any])
    let evidence = try Self.fixture("evidence/layer_excerpt.json")
    let keptSpace = try #require(evidence["coordinate_space"] as? [String: Any])
    let quoteSpace = try #require(said["coordinate_space"] as? [String: Any])
    #expect(Self.keys(quoteSpace) == Self.keys(keptSpace))
    #expect(quoteSpace["text"] as? String == keptSpace["text"] as? String)
    #expect(NSDictionary(dictionary: try #require(quoteSpace["rendition"] as? [String: Any])).isEqual(to: try #require(keptSpace["rendition"] as? [String: Any])))
    #expect(quoteSpace["width"] as? Double == 640)
    #expect((said["drawing"] as? [String: Any])?["space"] as? String == "excerpt_rendition")
    _ = try Self.files(said)
    _ = (w1, w2)
  }

  // MARK: Render cache

  @Test func theCacheKeepsOnlyTheVersionNamed() async throws {
    let cache = InkRenderCache(
      root: FileManager.default.temporaryDirectory.appendingPathComponent("ink-cache-\(UUID())"))
    let bytes = Data("a paper".utf8)
    let sha = InkHash.sha256(bytes)
    let url = try #require(cache.file(["backend": "demo-mac", "sha256": sha, "name": "paper.pdf"]))
    let token = try cache.begin(url, sha256: sha)
    cache.append(token, bytes.prefix(3))
    cache.append(token, bytes.dropFirst(3))
    let done = await withCheckedContinuation { c in cache.finish(token) { c.resume(returning: $0) } }
    #expect(try done.get() == url)
    #expect(try Data(contentsOf: url) == bytes)

    let wrong = try cache.begin(url, sha256: String(repeating: "b", count: 64))
    cache.append(wrong, bytes)
    let refused = await withCheckedContinuation { c in cache.finish(wrong) { c.resume(returning: $0) } }
    if case .failure(let error) = refused { #expect(error.code == "hash_mismatch") } else {
      Issue.record("a wrong hash was accepted")
    }
    // No climbing out of the cache.
    #expect(cache.file(["backend": "..", "sha256": sha, "name": "x"]) == nil)
    #expect(cache.file(["backend": "a", "sha256": sha, "name": "../x"]) == nil)
    #expect(cache.file(["backend": "a", "sha256": "nothex", "name": "x"]) == nil)
    cache.clear(backend: "demo-mac")
    #expect(!FileManager.default.fileExists(atPath: url.path))
  }

  /// The journal of drawings not yet saved: by hashes only, protected,
  /// never backed up, and a page's entry goes once its save is confirmed.
  @Test func theJournalKeepsAPageUntilItsSaveAndNothingElse() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("ink-journal-\(UUID())")
    let journal = InkJournal(root: root)
    let claktory = String(repeating: "c", count: 64), document = String(repeating: "d", count: 64)
    let folder = try #require(journal.folder(["claktory": claktory, "document": document]))
    // Only names the app made: hex, never a path.
    #expect(journal.folder(["claktory": "..", "document": document]) == nil)
    #expect(journal.folder(["claktory": claktory, "document": "paper.pdf"]) == nil)
    #expect(journal.folder(["claktory": claktory, "document": "a/b"]) == nil)

    let ink = PKDrawing(strokes: [Self.circle(at: CGPoint(x: 50, y: 50))]).dataRepresentation()
    let drawing: [String: Any] = [
      "format": "pencilkit", "content_version": 1, "bytes_base64": ink.base64EncodedString(),
      "sha256": InkHash.sha256(ink),
    ]
    func put(_ page: Int?, seq: Int) throws {
      try journal.put(
        folder: folder, page: page, seq: seq,
        fields: [
          "page_1based": page as Any, "source_sha256": String(repeating: "a", count: 64),
          "ink_layer_id": "layer-1", "base_revision": 2, "path": "/home/june/paper.pdf",
        ], drawing: drawing)
    }
    try put(3, seq: 4)
    try put(5, seq: 6)
    let entries = journal.list(folder: folder)
    #expect(entries.map { $0["page_1based"] as? Int } == [3, 5])
    let three = try #require(entries.first)
    #expect(three["seq"] as? Int == 4)
    #expect(three["ink_layer_id"] as? String == "layer-1")
    #expect(three["base_revision"] as? Int == 2)
    #expect((three["drawing"] as? [String: Any])?["bytes_base64"] as? String == ink.base64EncodedString())
    // Nothing it wasn't meant to keep: no path.
    #expect(three["path"] == nil)
    let file = folder.appendingPathComponent("page-3.json")
    let text = try String(contentsOf: file, encoding: .utf8)
    #expect(!text.contains("paper"))
    // The Simulator keeps no protection classes (it says none); on an iPad
    // the file is .complete.
    let attributes = try FileManager.default.attributesOfItem(atPath: file.path)
    #if targetEnvironment(simulator)
      #expect(attributes[.protectionKey] == nil || attributes[.protectionKey] as? FileProtectionType == .complete)
    #else
      #expect(attributes[.protectionKey] as? FileProtectionType == .complete)
    #endif
    #expect(try root.resourceValues(forKeys: [.isExcludedFromBackupKey]).isExcludedFromBackup == true)

    // A newer change replaces the page's entry; a save of an older one
    // leaves it.
    try put(3, seq: 9)
    journal.remove(folder: folder, page: 3, upto: 4)
    #expect(journal.list(folder: folder).first?["seq"] as? Int == 9)
    journal.remove(folder: folder, page: 3, upto: nil)
    journal.remove(folder: folder, page: 5, upto: nil)
    #expect(journal.list(folder: folder).isEmpty)
    // Nothing left behind.
    #expect(!FileManager.default.fileExists(atPath: folder.path))
    try? FileManager.default.removeItem(at: root)
  }

  @Test func theCacheKeepsWhatWasUsedLastWithinItsLimit() async throws {
    let cache = InkRenderCache(
      root: FileManager.default.temporaryDirectory.appendingPathComponent("ink-cache-\(UUID())"),
      limit: 2500)
    func keep(_ name: String, _ byte: UInt8) async throws -> URL {
      let bytes = Data(repeating: byte, count: 1000)
      let sha = InkHash.sha256(bytes)
      let url = try #require(cache.file(["backend": "b", "sha256": sha, "name": name]))
      let token = try cache.begin(url, sha256: sha)
      cache.append(token, bytes)
      _ = await withCheckedContinuation { c in cache.finish(token) { c.resume(returning: $0) } }
      // Distinct use times, older first.
      try await Task.sleep(nanoseconds: 1_100_000_000)
      return url
    }
    let a = try await keep("a.pdf", 1)
    let b = try await keep("b.pdf", 2)
    // a used again: b is now the one used longest ago.
    _ = await withCheckedContinuation { c in
      cache.handle(
        FlutterMethodCall(
          methodName: "cacheLookup",
          arguments: ["backend": "b", "sha256": InkHash.sha256(Data(repeating: 1, count: 1000)), "name": "a.pdf"])
      ) { c.resume(returning: $0) }
    }
    try await Task.sleep(nanoseconds: 1_100_000_000)
    let c = try await keep("c.pdf", 3)
    let exists = { (u: URL) in FileManager.default.fileExists(atPath: u.path) }
    #expect(exists(a) && exists(c))
    #expect(!exists(b))
    #expect(!FileManager.default.fileExists(atPath: b.deletingLastPathComponent().path))
  }
}
