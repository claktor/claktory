import Flutter
import PDFKit
import PencilKit
import Testing
import UIKit

@testable import Runner

/// The ink bridge's native side (app/ios/Runner/Ink) against the frozen
/// fixtures in app/test/ipad/fixtures/bridge, read by #filePath.
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
  static func paper() throws -> (URL, String) {
    let bounds = CGRect(x: 0, y: 0, width: 612, height: 792)
    let data = UIGraphicsPDFRenderer(bounds: bounds).pdfData { context in
      for n in 1...8 {
        context.beginPage()
        ("Page \(n)" as NSString).draw(
          at: CGPoint(x: 72, y: 72), withAttributes: [.font: UIFont.systemFont(ofSize: 24)])
      }
    }
    let doc = try #require(PDFDocument(data: data))
    doc.page(at: 4)?.rotation = 90
    doc.page(at: 6)?.setBounds(CGRect(x: 72, y: 144, width: 432, height: 504), for: .cropBox)
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
    #expect(Self.keys(ready) == Self.keys(fixture))
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
}
