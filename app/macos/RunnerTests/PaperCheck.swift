// Checks the Mac's PDF viewer's core (Runner/PaperCore.swift) and render
// cache (Runner/PaperCache.swift) without the app, against the iPad's
// bridge fixtures (app/test/ipad/fixtures/bridge), and that the viewer
// (Runner/PaperViewer.swift) never opens a link itself. Run it from
// app/macos: it reads the fixtures and that source file.
//
//   cd app/macos && swiftc -parse-as-library -o /tmp/paper-check \
//     Runner/PaperCore.swift Runner/PaperCache.swift \
//     RunnerTests/PaperCheck.swift && /tmp/paper-check
import AppKit
import Foundation
import PDFKit

private var failures = 0

private func check(_ ok: Bool, _ what: String, file: StaticString = #file, line: UInt = #line) {
  if ok {
    print("ok   \(what)")
  } else {
    failures += 1
    print("FAIL \(what) (line \(line))")
  }
}

private func fixture(_ name: String) -> [String: Any] {
  let url = URL(fileURLWithPath: "../test/ipad/fixtures/bridge/\(name)")
  guard let data = try? Data(contentsOf: url),
    let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
  else {
    print("FAIL can't read \(url.path): run from app/macos")
    exit(1)
  }
  return json
}

private func keys(_ any: Any?) -> Set<String> { Set((any as? [String: Any])?.keys ?? [:].keys) }

private let temp = FileManager.default.temporaryDirectory
  .appendingPathComponent("paper-check-\(UUID().uuidString)", isDirectory: true)

/// A Letter-size paper of [pages] pages, each with its number; "the
/// vanishing gradient" 100 pt from the top of pages 3 and 5; page 5 turned
/// 90°, page 7 cropped (as the iPad's tests and fixtures have it); a form
/// field on page 1.
private func paper(pages: Int, text: Bool = true) -> (URL, String) {
  let data = NSMutableData()
  var box = CGRect(x: 0, y: 0, width: 612, height: 792)
  let context = CGContext(consumer: CGDataConsumer(data: data as CFMutableData)!, mediaBox: &box, nil)!
  func draw(_ string: String, x: CGFloat, top: CGFloat, size: CGFloat) {
    let line = CTLineCreateWithAttributedString(
      NSAttributedString(string: string, attributes: [.font: NSFont.systemFont(ofSize: size)]))
    context.textPosition = CGPoint(x: x, y: 792 - top - size)
    CTLineDraw(line, context)
  }
  for n in 1...pages {
    context.beginPDFPage(nil)
    if text {
      draw("Page \(n)", x: 72, top: 72, size: 24)
      if n == 3 || n == 5 { draw("the vanishing gradient", x: 72, top: 100, size: 14) }
    }
    context.endPDFPage()
  }
  context.closePDF()
  let doc = PDFDocument(data: data as Data)!
  if pages >= 7 {
    doc.page(at: 4)?.rotation = 90
    doc.page(at: 6)?.setBounds(CGRect(x: 72, y: 144, width: 432, height: 504), for: .cropBox)
  }
  let field = PDFAnnotation(
    bounds: CGRect(x: 72, y: 600, width: 200, height: 24), forType: .widget, withProperties: nil)
  field.widgetFieldType = .text
  field.fieldName = "name"
  doc.page(at: 0)?.addAnnotation(field)
  let url = temp.appendingPathComponent("paper-\(UUID().uuidString).pdf")
  _ = doc.write(to: url)
  return (url, PaperHash.sha256(file: url)!)
}

/// Six pages with an outline as LaTeX writes one (/Dest and /GoTo, one
/// nested), written by hand: PDFKit doesn't write an outline set in code.
private func outlined() -> (URL, String) {
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
  let intro = add("I"), method = add("M"), grads = add("G"), results = add("R")
  objects[intro - 1] = "<< /Title (Introduction) /Parent 3 0 R /Next \(method) 0 R /Dest [\(pages[0]) 0 R /Fit] >>"
  objects[method - 1] =
    "<< /Title (Method) /Parent 3 0 R /Prev \(intro) 0 R /Next \(results) 0 R /First \(grads) 0 R /Last \(grads) 0 R /Count 1 /A << /S /GoTo /D [\(pages[2]) 0 R /Fit] >> >>"
  objects[grads - 1] = "<< /Title (Gradients) /Parent \(method) 0 R /Dest [\(pages[4]) 0 R /XYZ 0 792 0] >>"
  objects[results - 1] = "<< /Title (Results) /Parent 3 0 R /Prev \(method) 0 R /Dest [\(pages[4]) 0 R /Fit] >>"
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
  let url = temp.appendingPathComponent("outlined.pdf")
  try! Data(out.utf8).write(to: url)
  return (url, PaperHash.sha256(file: url)!)
}

/// Waits for [done] to be called on the main thread.
private func wait<T>(_ start: (@escaping (T) -> Void) -> Void) -> T {
  var value: T?
  start { value = $0 }
  let end = Date().addingTimeInterval(10)
  while value == nil, Date() < end { RunLoop.main.run(until: Date().addingTimeInterval(0.01)) }
  return value!
}

/// A window with a PDF view and a text field beside it, the keys in one or
/// the other.
private func keys() {
  _ = NSApplication.shared
  let window = NSWindow(
    contentRect: NSRect(x: 0, y: 0, width: 800, height: 600), styleMask: [.titled],
    backing: .buffered, defer: true)
  let root = NSView(frame: window.contentLayoutRect)
  window.contentView = root
  let pdf = PaperPDFView(frame: NSRect(x: 0, y: 0, width: 400, height: 600))
  let (file, _) = paper(pages: 2)
  pdf.document = PDFDocument(url: file)
  let box = NSTextField(frame: NSRect(x: 420, y: 280, width: 300, height: 24))
  root.addSubview(pdf)
  root.addSubview(box)
  var asked: [Bool?] = []
  pdf.onFind = { asked.append($0) }
  func press(_ key: String, shift: Bool = false) -> Bool {
    let event = NSEvent.keyEvent(
      with: .keyDown, location: .zero, modifierFlags: shift ? [.command, .shift] : [.command],
      timestamp: 0, windowNumber: window.windowNumber, context: nil,
      characters: shift ? key.uppercased() : key, charactersIgnoringModifiers: key,
      isARepeat: false, keyCode: key == "f" ? 3 : 5)!
    return pdf.performKeyEquivalent(with: event)
  }
  check(window.makeFirstResponder(box), "the text field takes the keys")
  _ = press("f")
  _ = press("g")
  check(asked.isEmpty, "⌘F and ⌘G in a field beside the page are the field's \(asked)")
  let inside = pdf.documentView ?? pdf
  check(window.makeFirstResponder(inside.acceptsFirstResponder ? inside : pdf), "the page takes the keys")
  check(press("f") && press("g") && press("g", shift: true), "the page's ⌘F, ⌘G and ⇧⌘G are taken")
  check(asked.count == 3 && asked[0] == nil && asked[1] == true && asked[2] == false, "as find, next and back \(asked)")
}

@main
struct PaperCheck {
  static func main() {
    try! FileManager.default.createDirectory(at: temp, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: temp) }

    // Opening: the version named, its pages, as ready's fixture says.
    let (file, sha) = paper(pages: 8)
    guard case .success(let (doc, actual)) = PaperCore.open(path: file.path, expected: sha) else {
      check(false, "a paper opens")
      exit(1)
    }
    check(actual == sha, "the version opened is the one named")
    let ready = PaperCore.describe(doc, sha256: sha)
    let readyFixture = fixture("ev_ready.json")["arguments"] as! [String: Any]
    check(keys(ready) == keys(readyFixture), "ready has the fixture's fields")
    let pages = ready["pages"] as! [[String: Any]]
    check(keys(pages[0]) == keys((readyFixture["pages"] as! [[String: Any]])[0]), "pages have the fixture's fields")
    check(pages[4]["rotation"] as? Int == 90, "a turned page says so")
    check(pages[6]["crop_box"] as? [Double] == [72, 144, 432, 504], "a cropped page says its crop")
    check(ready["drawing_space"] as? String == "none", "nothing is drawn on a Mac")
    check(ready["outline"] == nil, "no outline, no outline field")

    if case .failure(let refusal) = PaperCore.open(path: file.path, expected: String(repeating: "a", count: 64)) {
      check(refusal.code == "document_hash_mismatch", "another version is refused")
      check(refusal.fields["actual_sha256"] as? String == sha, "and says which it is")
    } else {
      check(false, "another version is refused")
    }
    let (long, longSha) = paper(pages: 2001, text: false)
    if case .failure(let refusal) = PaperCore.open(path: long.path, expected: longSha) {
      check(refusal.code == "document_too_large", "more than 2000 pages is refused")
      check(
        keys(refusal.fields) == keys(fixture("ev_error_document_too_large.json")["arguments"]),
        "as the fixture says")
      check(refusal.fields["page_count"] as? Int == 2001, "with the count")
    } else {
      check(false, "more than 2000 pages is refused")
    }

    // The outline, from a file as LaTeX writes one.
    let (outline, outlineSha) = outlined()
    if case .success(let (odoc, _)) = PaperCore.open(path: outline.path, expected: outlineSha) {
      let entries = PaperCore.describe(odoc, sha256: outlineSha)["outline"] as? [[String: Any]] ?? []
      check(entries.map { $0["title"] as? String ?? "" } == ["Introduction", "Method", "Gradients", "Results"], "the outline, depth first")
      check(entries.map { $0["page_1based"] as? Int ?? 0 } == [1, 3, 5, 5], "each entry's page")
      check(entries.map { $0["depth"] as? Int ?? -1 } == [0, 0, 1, 0], "and depth")
      let first = (fixture("ev_ready_outline.json")["arguments"] as! [String: Any])["outline"] as! [[String: Any]]
      check(keys(entries.first) == keys(first[0]), "entries have the fixture's fields")
    } else {
      check(false, "an outlined paper opens")
    }

    // Search: regions on their pages, as cmd_search.json.
    let found = PaperCore.search(doc, query: "Vanishing Gradient", limit: 200)
    let answer = PaperCore.searchJSON(query: "vanishing gradient", matches: found.matches, truncated: found.truncated)
    let searchFixture = fixture("cmd_search.json")["result"] as! [String: Any]
    check(keys(answer) == keys(searchFixture), "a search answers as the fixture")
    let matches = answer["matches"] as! [[String: Any]]
    check(matches.map { $0["page_1based"] as? Int ?? 0 } == [3, 5], "on pages 3 and 5")
    check(keys(matches.first) == keys((searchFixture["matches"] as! [[String: Any]])[0]), "matches have the fixture's fields")
    if let r = PaperCore.region(matches.first?["region"]) {
      // After "the ", 100 pt down.
      check(r.minX > 72.0 / 612 && r.minX < 110.0 / 612 && abs(r.minY - 100.0 / 792) < 0.02, "a match's region is where its words are \(r)")
    } else {
      check(false, "a match has a region")
    }
    check((matches.first?["text"] as? String)?.contains("vanishing gradient") == true, "with its line")
    let one = PaperCore.search(doc, query: "page", limit: 3)
    check(one.matches.count == 3 && one.truncated, "a search stops at its limit, and says so")

    // Regions and page space, both ways, on a cropped page.
    let cropped = doc.page(at: 6)!
    let region = CGRect(x: 0.25, y: 0.5, width: 0.5, height: 0.25)
    let back = PaperCore.region(PaperCore.pageRect(region, on: cropped), on: cropped)
    check(abs(back.minX - 0.25) < 1e-6 && abs(back.minY - 0.5) < 1e-6 && abs(back.width - 0.5) < 1e-6, "a region maps to its page and back")
    check(PaperCore.pageRect(CGRect(x: 0, y: 0, width: 1, height: 1), on: cropped) == CGRect(x: 72, y: 144, width: 432, height: 504), "a whole region is the crop box")

    // Forms are shown, never filled in.
    PaperCore.readOnlyForms(doc)
    let widgets = doc.page(at: 0)!.annotations.filter { $0.type == "Widget" }
    check(!widgets.isEmpty && widgets.allSatisfy { $0.isReadOnly }, "form fields are read-only")

    // The render cache: kept only as the version named, this user's only,
    // and the copies used longest ago go first.
    let cache = PaperCache(root: temp.appendingPathComponent("cache"), limit: 3000)
    check(cache.file(backend: "../x", sha256: sha, name: "a.pdf") == nil, "no path outside the cache")
    let bytes = try! Data(contentsOf: file)
    let target = cache.file(backend: "mac", sha256: sha, name: "paper.pdf")!
    let token = try! cache.begin(target, sha256: sha)
    cache.append(token, bytes.prefix(1000))
    cache.append(token, bytes.dropFirst(1000))
    let kept: Result<URL, PaperCacheError> = wait { done in cache.finish(token, done: done) }
    if case .success(let url) = kept {
      check(PaperHash.sha256(file: url) == sha, "a copy is kept as the version named")
      for folder in [cache.root, url.deletingLastPathComponent(), url.deletingLastPathComponent().deletingLastPathComponent()] {
        let mode = (try? FileManager.default.attributesOfItem(atPath: folder.path)[.posixPermissions] as? Int) ?? 0
        check(mode == 0o700, "\(folder.lastPathComponent) is this user's only (\(String(mode, radix: 8)))")
      }
      check(cache.lookup(url) != nil, "and found again")
    } else {
      check(false, "a copy is kept")
    }
    let wrong = cache.file(backend: "mac", sha256: String(repeating: "b", count: 64), name: "paper.pdf")!
    let other = try! cache.begin(wrong, sha256: String(repeating: "b", count: 64))
    cache.append(other, bytes)
    let refused: Result<URL, PaperCacheError> = wait { done in cache.finish(other, done: done) }
    if case .failure(let error) = refused {
      check(error.code == "hash_mismatch", "another version isn't kept")
      check(!FileManager.default.fileExists(atPath: wrong.path), "nor left behind")
      let partials = (try? FileManager.default.contentsOfDirectory(atPath: wrong.deletingLastPathComponent().path)) ?? []
      check(partials.isEmpty, "not even in part")
    } else {
      check(false, "another version isn't kept")
    }
    // Three copies of 1000 bytes in a 3000-byte cache, then a fourth: the
    // one used longest ago goes, the others stay.
    let small = PaperCache(root: temp.appendingPathComponent("small"), limit: 3000)
    func keep(_ name: String, _ byte: UInt8) -> URL {
      let data = Data(repeating: byte, count: 1000)
      let sha = PaperHash.sha256(data)
      let url = small.file(backend: "mac", sha256: sha, name: name)!
      let token = try! small.begin(url, sha256: sha)
      small.append(token, data)
      let result: Result<URL, PaperCacheError> = wait { done in small.finish(token, done: done) }
      return try! result.get()
    }
    let a = keep("a", 1)
    Thread.sleep(forTimeInterval: 1.1)
    let b = keep("b", 2)
    Thread.sleep(forTimeInterval: 1.1)
    let c = keep("c", 3)
    Thread.sleep(forTimeInterval: 1.1)
    _ = small.lookup(a)  // a used again: b is now the oldest
    Thread.sleep(forTimeInterval: 1.1)
    let d = keep("d", 4)
    let here = [a, b, c, d].map { FileManager.default.fileExists(atPath: $0.path) }
    check(here == [true, false, true, true], "the copy used longest ago goes first \(here)")

    // The viewer never opens a link itself: it only tells Dart.
    let viewer = (try? String(contentsOfFile: "Runner/PaperViewer.swift", encoding: .utf8)) ?? ""
    check(viewer.contains("func pdfViewWillClick(onLink"), "links are the delegate's")
    check(!viewer.contains("NSWorkspace"), "and nothing in the viewer opens one")
    check(!viewer.contains("super.pdfViewWillClick"), "not even PDFKit's own way")

    // ⌘F, ⌘G and ⇧⌘G are the page's only while it has the keys: a message
    // box beside it keeps its own.
    keys()

    print(failures == 0 ? "All checks passed." : "\(failures) failed.")
    exit(failures == 0 ? 0 : 1)
  }
}
