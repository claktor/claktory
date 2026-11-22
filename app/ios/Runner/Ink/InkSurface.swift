// What every document surface shares (InkView, PaperView): the channel, the
// ink layers it holds per page, coalesced drawingChanged events, its own
// undo, tools and the Pencil's double-tap, interruptions (layout, the app
// leaving the screen, memory warnings) and handwriting (at the end).

import Flutter
import PencilKit
import UIKit

class InkSurface: NSObject, FlutterPlatformView, PKCanvasViewDelegate,
  UIPencilInteractionDelegate
{
  let root = InkRootView()
  let params: InkParams
  let channel: InkEvents
  /// One undo history per surface, across its pages.
  let undo = UndoManager()

  /// An ink layer as this surface knows it: the backend's id and the
  /// revision last saved (null/0 before the first save), the drawing now
  /// on screen, and what Dart last heard.
  struct Layer {
    var inkLayerId: String?
    var revision: Int?
    var drawing = PKDrawing()
    var sentStrokes = 0
    var dirty = false
    var cause: String?
    /// The last drawingChanged sent for it; nil while it's as loaded.
    var sentSeq: Int?
  }

  /// By page (1-based); 0 for a surface with one drawing.
  var layers: [Int: Layer] = [:]
  private var localSeq = 0
  private var flushWork: DispatchWorkItem?
  /// Set while this surface assigns a drawing itself, so the change
  /// PencilKit then reports isn't sent as the person's (spike: a fresh
  /// overlay reports an empty "change" on load).
  var loading = 0
  /// The page whose canvas a tool is drawing on now, if any.
  private(set) var activePage: Int?
  /// The page's drawing when the tool went down: whether an interrupted
  /// stroke left anything behind.
  private var drawingAtBegin: PKDrawing?
  private var lifecycle: [NSObjectProtocol] = []
  /// Background time asked for while a flushed change waits to be saved.
  private var backgroundTask: UIBackgroundTaskIdentifier = .invalid
  private var awaitingSave: Set<Int> = []
  /// PKStrokeRecognizers by page and languages (Handwriting); a cache.
  var recognizers: [String: AnyObject] = [:]
  /// What VoiceOver reads first: where you are ("Page 3 of 12") and
  /// whether your drawing is there; the document's own content follows.
  let status = UIAccessibilityElement(accessibilityContainer: NSObject())
  private var attached = false
  private var queued: [(String, Any?)] = []
  var tool: PKTool = PKInkingTool(.pen, color: .black, width: 3)
  private var toolState: [String: Any] = ["tool": "pen", "color": "#000000FF", "width": 3.0]
  private var previousTool: (PKTool, [String: Any])?
  var drawingPolicy: PKCanvasViewDrawingPolicy
  /// Drawing turned off ("none"): the document is only read.
  var drawingOff: Bool
  /// The version shown: the creation params', until a PDF is reloaded.
  var sourceSha256: String?

  init(frame: CGRect, params: InkParams, channel: InkEvents) {
    self.params = params
    self.channel = channel
    drawingPolicy = params.drawingPolicy
    drawingOff = params.drawingOff
    sourceSha256 = params.sourceSha256
    super.init()
    root.frame = frame
    root.backgroundColor = params.paper
    root.overrideUserInterfaceStyle = params.style
    root.clipsToBounds = true
    root.onResize = { [weak self] _ in self?.interrupt("resize") }
    for ref in params.layers {
      let page = ref["page_1based"] as? Int ?? 0
      layers[page] = Layer(
        inkLayerId: ref["ink_layer_id"] as? String, revision: ref["revision"] as? Int)
    }
    status.accessibilityContainer = root
    root.status = status
    status.accessibilityTraits = [.staticText, .updatesFrequently]
    let pencil = UIPencilInteraction()
    pencil.delegate = self
    root.addInteraction(pencil)
    hoverLayer.isHidden = true
    hoverLayer.zPosition = 1000
    root.layer.addSublayer(hoverLayer)
    let hover = UIHoverGestureRecognizer(target: self, action: #selector(hovered(_:)))
    hover.cancelsTouchesInView = false
    root.addGestureRecognizer(hover)
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else { return result(nil) }
      self.handle(call, result)
    }
    observeLifecycle()
  }

  // MARK: Accessibility

  /// The status's words, and the page whose drawing it speaks of; nil
  /// until there's a document.
  var statusText: (label: String, page: Int)? { nil }

  /// The surface's elements, in reading order: the status, then these.
  func readingOrder() -> [Any] { [] }

  func refreshStatus() {
    guard let text = statusText else {
      root.accessibilityElements = nil
      return
    }
    status.accessibilityLabel = text.label
    let drawn = layers[text.page]?.drawing.strokes.isEmpty == false
    status.accessibilityValue = drawn ? "You drew on this page" : nil
    root.accessibilityElements = [status] + readingOrder()
  }

  deinit {
    flushWork?.cancel()
    lifecycle.forEach(NotificationCenter.default.removeObserver)
    endBackgroundTask()
  }

  func view() -> UIView { root }

  // MARK: Events

  /// Sends an event once Dart has attached; until then they wait, so the
  /// first ones (ready, an error) aren't lost before Dart listens.
  func send(_ method: String, _ arguments: Any?) {
    if attached {
      channel.invokeMethod(method, arguments: arguments)
    } else {
      queued.append((method, arguments))
    }
  }

  func error(_ fields: [String: Any]) {
    send("error", fields)
  }

  // MARK: Commands

  func handle(_ call: FlutterMethodCall, _ result: @escaping FlutterResult) {
    let args = call.arguments as? [String: Any] ?? [:]
    switch call.method {
    case "attach":
      attached = true
      let waiting = queued
      queued = []
      for (method, arguments) in waiting { channel.invokeMethod(method, arguments: arguments) }
      result(nil)
    case "setTool":
      result(setTool(args))
    case "setDrawingPolicy":
      let name = args["drawing_policy"] as? String ?? "default"
      if name == "none" {
        // A stroke in progress ends here, and isn't kept.
        interrupt("drawing_off")
        drawingOff = true
      } else {
        drawingOff = false
        drawingPolicy = name == "pencil_only" ? .pencilOnly : name == "any_input" ? .anyInput : .default
      }
      for canvas in liveCanvases { applyDrawing(canvas) }
      drawingPolicyChanged()
      result(["drawing_policy": name])
    case "showToolPicker":
      // Kept for completeness; the workbench uses its own controls,
      // because PKToolPicker takes focus from Flutter's text fields.
      result(["visible": false])
    case "undo":
      undoChange(redo: false)
      result(undoState)
    case "redo":
      undoChange(redo: true)
      result(undoState)
    case "loadDrawing":
      result(load(args["layers"] as? [[String: Any]] ?? []))
    case "layerSaved":
      let page = args["page_1based"] as? Int ?? 0
      var layer = layers[page] ?? Layer()
      layer.inkLayerId = args["ink_layer_id"] as? String
      layer.revision = args["revision"] as? Int
      layers[page] = layer
      awaitingSave.remove(page)
      if awaitingSave.isEmpty { endBackgroundTask() }
      result([String: Any]())
    case "setActive":
      let active = args["active"] as? Bool ?? true
      if !active {
        interrupt("hidden")
        flush()
      }
      setActive(active)
      result(["active": active, "pending_flushed": !active])
    case "exportComposite":
      exportComposite(args, result)
    case "exportLayer":
      exportLayer(args, result)
    case "recognizeText":
      recognizeText(args, result)
    default:
      result(FlutterMethodNotImplemented)
    }
  }

  // MARK: Subclasses

  /// After setDrawingPolicy, for what isn't a canvas's (PDFKit's own
  /// gestures).
  func drawingPolicyChanged() {}

  /// The canvases on screen now, to give a new tool or policy to.
  var liveCanvases: [InkCanvas] { [] }

  /// Puts a page's drawing on its canvas, if it has one now.
  func show(_ drawing: PKDrawing, page: Int) {}

  /// The page's size in its drawing's own units, to normalize bounds.
  func drawingSize(page: Int) -> CGSize { .zero }

  func setActive(_ active: Bool) {}

  func exportComposite(_ args: [String: Any], _ result: @escaping FlutterResult) {
    result(FlutterError(code: "unsupported", message: nil, details: nil))
  }

  /// Where a page's drawing lies in its document: the layer's
  /// `coordinate_space`, its `space` and `drawing_to_document`, the
  /// drawing's own size (its units) and how the document is turned on
  /// screen (a PDF page's /Rotate).
  struct LayerGeometry {
    let coordinateSpace: [String: Any]
    let space: String
    let drawingToDocument: [Double]
    let size: CGSize
    let rotation: Int
  }

  /// [page]'s geometry (0: the one drawing), or nil if there's no such
  /// page or the document isn't open yet.
  func layerGeometry(page: Int) -> LayerGeometry? { nil }

  // MARK: Saving (cmd_export_layer)

  /// A page's drawing as it is now, for Dart to save as its ink layer's
  /// next revision: the drawing and an ink-only preview as temporary files
  /// (never through the channel), and where they lie in the document. A
  /// change not yet sent is sent first, so `local_seq` names the
  /// drawingChanged this export is.
  func exportLayer(_ args: [String: Any], _ result: @escaping FlutterResult) {
    let page = args["page_1based"] as? Int ?? 0
    if layers[page]?.dirty == true { flush() }
    guard let layer = layers[page],
      layer.inkLayerId != nil || layer.sentSeq != nil || !layer.drawing.strokes.isEmpty
    else {
      return result(FlutterError(code: "nothing_drawn", message: nil, details: nil))
    }
    guard let geometry = layerGeometry(page: page) else {
      return result(FlutterError(code: "bad_request", message: "page", details: nil))
    }
    let drawing = layer.drawing
    let data = drawing.dataRepresentation()
    let maxLongEdge = CGFloat((args["preview_max_long_edge"] as? NSNumber)?.doubleValue ?? 2048)
    let maxBytes = (args["preview_max_bytes"] as? NSNumber)?.intValue ?? 4 * 1024 * 1024
    // PencilKit renders on the main thread.
    guard
      let preview = InkExport.preview(
        drawing, size: geometry.size, rotation: geometry.rotation, maxLongEdge: maxLongEdge,
        maxBytes: maxBytes),
      let drawingFile = InkExport.write(data, ext: "pkdrawing"),
      let previewFile = InkExport.write(preview.png, ext: "png")
    else { return result(FlutterError(code: "export_too_large", message: nil, details: nil)) }
    result([
      "page_1based": pageJSON(page),
      "ink_layer_id": layer.inkLayerId.inkJSON,
      "base_revision": layer.inkLayerId == nil ? NSNull() : layer.revision.inkJSON,
      "local_seq": layer.sentSeq.inkJSON,
      "source_sha256": sourceSha256.inkJSON,
      "stroke_count": drawing.strokes.count,
      "coordinate_space": geometry.coordinateSpace,
      "drawing": [
        "format": "pencilkit", "content_version": drawing.inkContentVersion,
        "space": geometry.space, "drawing_to_document": geometry.drawingToDocument,
        "file": drawingFile.path, "sha256": InkHash.sha256(data), "bytes": data.count,
      ],
      "ink_preview": [
        "file": previewFile.path, "mime": "image/png", "sha256": InkHash.sha256(preview.png),
        "pixel_width": preview.width, "pixel_height": preview.height, "bytes": preview.png.count,
      ],
    ])
  }

  // MARK: Loading

  private func load(_ refs: [[String: Any]]) -> [String: Any] {
    var loaded: [[String: Any]] = []
    var unreadable: [[String: Any]] = []
    var kept = false
    for ref in refs {
      let page = ref["page_1based"] as? Int ?? 0
      let wire = ref["drawing"] as? [String: Any] ?? [:]
      var entry: [String: Any] = [
        "page_1based": ref["page_1based"] ?? NSNull(),
        "ink_layer_id": ref["ink_layer_id"] ?? NSNull(), "revision": ref["revision"] ?? NSNull(),
      ]
      let version = wire["content_version"] as? Int ?? 1
      // Its layer deleted elsewhere: the page shows nothing, has no layer.
      if ref["clear"] as? Bool == true {
        var layer = layers[page] ?? Layer()
        layer.inkLayerId = nil
        layer.revision = nil
        layer.drawing = PKDrawing()
        layer.sentStrokes = 0
        layer.dirty = false
        layer.sentSeq = nil
        layers[page] = layer
        loading += 1
        show(layer.drawing, page: page)
        loading -= 1
        entry["strokes"] = 0
        loaded.append(entry)
        continue
      }
      guard let base64 = wire["bytes_base64"] as? String, let data = Data(base64Encoded: base64),
        wire["sha256"] as? String == nil || InkHash.sha256(data) == wire["sha256"] as? String,
        version <= PKDrawing.inkMaxContentVersion,
        let drawing = try? PKDrawing(data: data)
      else {
        entry["content_version"] = version
        entry["max_content_version"] = PKDrawing.inkMaxContentVersion
        unreadable.append(entry)
        error([
          "code": "drawing_unreadable", "recoverable": false,
          "page_1based": ref["page_1based"] ?? NSNull(),
          "ink_layer_id": ref["ink_layer_id"] ?? NSNull(), "content_version": version,
          "max_content_version": PKDrawing.inkMaxContentVersion,
        ])
        continue
      }
      var layer = layers[page] ?? Layer()
      // Keep both, after another device saved the page first: theirs, with
      // what's on the page now on top, as the layer's next revision (a
      // change to send, its new strokes the person's).
      let keepMine = ref["keep_mine"] as? Bool == true
      let shown = keepMine ? drawing.appending(layer.drawing) : drawing
      layer.inkLayerId = ref["ink_layer_id"] as? String
      layer.revision = ref["revision"] as? Int
      layer.drawing = shown
      layer.sentStrokes = drawing.strokes.count
      layer.dirty = keepMine
      layer.cause = keepMine ? "keep_both" : nil
      if !keepMine { layer.sentSeq = nil }
      kept = kept || keepMine
      layers[page] = layer
      loading += 1
      show(shown, page: page)
      loading -= 1
      entry["strokes"] = shown.strokes.count
      loaded.append(entry)
    }
    // A loaded drawing isn't something to undo.
    undo.removeAllActions()
    refreshStatus()
    send("undoState", undoState)
    if kept { scheduleFlush() }
    return ["loaded": loaded, "unreadable": unreadable]
  }

  /// A new canvas for a page (or the surface's only one), with this
  /// surface's tool, policy, undo and drawing.
  /// The policy on a canvas, and whether it draws at all. A surface whose
  /// canvas holds the document (InkView) keeps scrolling with drawing off.
  func applyDrawing(_ canvas: InkCanvas) {
    canvas.drawingPolicy = drawingPolicy
    canvas.draws = !drawingOff
  }

  func makeCanvas(page: Int) -> InkCanvas {
    let canvas = InkCanvas()
    canvas.surfaceUndo = undo
    canvas.backgroundColor = .clear
    canvas.isOpaque = false
    applyDrawing(canvas)
    canvas.tool = tool
    canvas.tag = page
    canvas.overrideUserInterfaceStyle = params.style
    loading += 1
    canvas.drawing = layers[page]?.drawing ?? PKDrawing()
    loading -= 1
    canvas.delegate = self
    return canvas
  }

  // MARK: Tools

  private func setTool(_ args: [String: Any]) -> [String: Any] {
    let name = args["tool"] as? String ?? "pen"
    let color = UIColor(inkHex: args["color"] as? String) ?? .black
    let width = CGFloat((args["width"] as? NSNumber)?.doubleValue ?? 3)
    previousTool = (tool, toolState)
    tool = Self.tool(name, color: color, width: width)
    toolState = ["tool": name, "color": args["color"] ?? NSNull(), "width": Double(width)]
    for canvas in liveCanvases { canvas.tool = tool }
    return toolState
  }

  static func tool(_ name: String, color: UIColor, width: CGFloat) -> PKTool {
    switch name {
    case "pencil": return PKInkingTool(.pencil, color: color, width: width)
    case "marker": return PKInkingTool(.marker, color: color, width: width)
    case "eraser":
      if #available(iOS 16.4, *) { return PKEraserTool(.vector, width: width) }
      return PKEraserTool(.vector)
    case "lasso": return PKLassoTool()
    default: return PKInkingTool(.pen, color: color, width: width)
    }
  }

  /// Apple Pencil's double-tap and squeeze: what the person chose in
  /// Settings, and only the non-destructive tools (never send, accept or
  /// delete: §2). The tool changes here and Dart hears it (toolChanged);
  /// a palette is Dart's to show (pencilAction), near the Pencil.
  func pencilInteractionDidTap(_ interaction: UIPencilInteraction) {
    perform(UIPencilInteraction.preferredTapAction, source: "pencil_double_tap", at: nil)
  }

  @available(iOS 17.5, *)
  func pencilInteraction(_ interaction: UIPencilInteraction, didReceiveTap tap: UIPencilInteraction.Tap) {
    perform(UIPencilInteraction.preferredTapAction, source: "pencil_double_tap", at: tap.hoverPose?.location)
  }

  @available(iOS 17.5, *)
  func pencilInteraction(
    _ interaction: UIPencilInteraction, didReceiveSqueeze squeeze: UIPencilInteraction.Squeeze
  ) {
    // Once, when the squeeze ends.
    guard squeeze.phase == .ended else { return }
    perform(UIPencilInteraction.preferredSqueezeAction, source: "pencil_squeeze", at: squeeze.hoverPose?.location)
  }

  /// What a Pencil gesture does. `at`: where the Pencil hovered, in the
  /// surface's view, if it did.
  func perform(_ action: UIPencilPreferredAction, source: String, at point: CGPoint?) {
    switch action {
    case .switchEraser:
      if toolState["tool"] as? String == "eraser", let previous = previousTool {
        swap(to: previous, source: source)
      } else {
        previousTool = (tool, toolState)
        tool = Self.tool("eraser", color: .black, width: 12)
        toolState = ["tool": "eraser", "color": NSNull(), "width": 12.0]
        applyTool(source: source)
      }
    case .switchPrevious:
      if let previous = previousTool { swap(to: previous, source: source) }
    case .showColorPalette, .showInkAttributes:
      palette(action == .showColorPalette ? "show_color_palette" : "show_ink_attributes", source, point)
    default:
      if #available(iOS 17.5, *), action == .showContextualPalette {
        palette("show_contextual_palette", source, point)
      }
      // Ignore, and the system's own shortcut (which the system runs).
    }
  }

  private func palette(_ action: String, _ source: String, _ point: CGPoint?) {
    let size = root.bounds.size
    let at: Any =
      point.flatMap { p -> [String: Double]? in
        guard size.width > 0, size.height > 0 else { return nil }
        return InkRegion.json(CGRect(x: p.x / size.width, y: p.y / size.height, width: 0, height: 0))
          .filter { $0.key == "x" || $0.key == "y" }
      } ?? NSNull()
    send("pencilAction", ["action": action, "source": source, "at": at])
  }

  private func swap(to previous: (PKTool, [String: Any]), source: String) {
    let current = (tool, toolState)
    (tool, toolState) = previous
    previousTool = current
    applyTool(source: source)
  }

  private func applyTool(source: String) {
    for canvas in liveCanvases { canvas.tool = tool }
    var event = toolState
    event["source"] = source
    send("toolChanged", event)
    hideHover()
  }

  // MARK: Hover (iPadOS 16.1+, a Pencil that hovers)

  /// The mark the tool would make, previewed under a hovering Pencil:
  /// its footprint and colour at the page's zoom, steady whatever the
  /// height (HIG), never drawn into the ink. Not for a pointer.
  let hoverLayer = CAShapeLayer()

  /// Drawing units to the root view's points, where the Pencil hovers.
  func zoom(at point: CGPoint) -> CGFloat { 1 }

  struct HoverPreview: Equatable {
    let center: CGPoint
    let diameter: CGFloat
    let fill: UIColor?
    let outline: Bool
  }

  /// What to preview at a point, or nothing: no tool to preview (lasso),
  /// drawing off, or a stroke under way.
  func hoverPreview(at point: CGPoint) -> HoverPreview? {
    guard !drawingOff, activePage == nil else { return nil }
    let name = toolState["tool"] as? String ?? "pen"
    guard name != "lasso" else { return nil }
    let width = CGFloat((toolState["width"] as? NSNumber)?.doubleValue ?? 3)
    // At least a few points, so a fine pen still shows where it lands.
    let diameter = max(4, width * zoom(at: point))
    if name == "eraser" {
      return HoverPreview(center: point, diameter: diameter, fill: nil, outline: true)
    }
    let color = UIColor(inkHex: toolState["color"] as? String) ?? .black
    return HoverPreview(center: point, diameter: diameter, fill: color, outline: false)
  }

  func showHover(at point: CGPoint) {
    guard let preview = hoverPreview(at: point) else { return hideHover() }
    CATransaction.begin()
    CATransaction.setDisableActions(true)
    let r = preview.diameter / 2
    hoverLayer.path = UIBezierPath(
      ovalIn: CGRect(x: preview.center.x - r, y: preview.center.y - r, width: 2 * r, height: 2 * r)
    ).cgPath
    // The ink's exact colour, half seen; an eraser as a ring.
    hoverLayer.fillColor = preview.fill?.withAlphaComponent((preview.fill?.cgColor.alpha ?? 1) * 0.5).cgColor
    hoverLayer.strokeColor = preview.outline ? UIColor(white: 0.35, alpha: 0.9).cgColor : nil
    hoverLayer.lineWidth = preview.outline ? 1 : 0
    hoverLayer.isHidden = false
    CATransaction.commit()
  }

  func hideHover() {
    hoverLayer.isHidden = true
  }

  @objc func hovered(_ recognizer: UIHoverGestureRecognizer) {
    switch recognizer.state {
    case .began, .changed:
      // A Pencil reports how high it is; a pointer doesn't.
      if #available(iOS 16.1, *), recognizer.zOffset > 0 {
        showHover(at: recognizer.location(in: root))
      } else {
        hideHover()
      }
    default:
      hideHover()
    }
  }

  // MARK: Undo

  var undoState: [String: Any] { ["can_undo": undo.canUndo, "can_redo": undo.canRedo] }

  private var undoing: String?

  private func undoChange(redo: Bool) {
    interrupt("undo")
    undoing = redo ? "redo" : "undo"
    if redo { undo.redo() } else { undo.undo() }
    undoing = nil
  }

  // MARK: PencilKit

  func canvasViewDidBeginUsingTool(_ canvasView: PKCanvasView) {
    hideHover()
    activePage = canvasView.tag
    drawingAtBegin = canvasView.drawing
    send("strokeActive", ["page_1based": pageJSON(canvasView.tag), "active": true])
  }

  func canvasViewDidEndUsingTool(_ canvasView: PKCanvasView) {
    guard activePage != nil else { return }
    activePage = nil
    send("strokeActive", ["page_1based": pageJSON(canvasView.tag), "active": false])
    scheduleFlush()
  }

  func canvasViewDrawingDidChange(_ canvasView: PKCanvasView) {
    let page = canvasView.tag
    var layer = layers[page] ?? Layer()
    if loading > 0 || canvasView.drawing == layer.drawing {
      layer.drawing = canvasView.drawing
      layers[page] = layer
      return
    }
    let before = layer.drawing.strokes.count
    layer.drawing = canvasView.drawing
    layer.dirty = true
    let now = layer.drawing.strokes.count
    layer.cause =
      undoing ?? (now > before ? "stroke" : now < before ? "erase" : (layer.cause ?? "lasso"))
    layers[page] = layer
    send("undoState", undoState)
    if before == 0 || now == 0 { refreshStatus() }
    if activePage == nil { scheduleFlush() }
  }

  /// A layout change, rotation, undo, hiding or the app leaving the screen
  /// while a tool is down: the stroke is cancelled here, so none is ever
  /// committed half in one layout and half in another. Dart hears whether
  /// anything was kept (PencilKit may have committed it first, in the
  /// page's own space), after the stroke's end; a kept change follows.
  func interrupt(_ reason: String) {
    guard let page = activePage else { return }
    activePage = nil
    let canvas = liveCanvases.first { $0.tag == page }
    canvas?.cancelStroke()
    let kept = canvas.map { $0.drawing != (drawingAtBegin ?? $0.drawing) } ?? false
    drawingAtBegin = nil
    send("strokeActive", ["page_1based": pageJSON(page), "active": false])
    send("strokeInterrupted", ["page_1based": pageJSON(page), "reason": reason, "kept": kept])
    if layers.values.contains(where: \.dirty) { scheduleFlush() }
  }

  // MARK: Leaving the screen, memory

  private func observeLifecycle() {
    let center = NotificationCenter.default
    func on(_ name: Notification.Name, _ act: @escaping (InkSurface, Notification) -> Void) {
      lifecycle.append(
        center.addObserver(forName: name, object: nil, queue: .main) { [weak self] note in
          if let self { act(self, note) }
        })
    }
    // A window losing the screen (the app switcher, Control Center, another
    // app in Stage Manager, locking): touches end there, so does a stroke.
    on(UIScene.willDeactivateNotification) { surface, note in
      guard surface.isMine(note.object) else { return }
      surface.leaveScreen()
    }
    on(UIScene.didEnterBackgroundNotification) { surface, note in
      guard surface.isMine(note.object) else { return }
      surface.leaveScreen(background: true)
    }
    on(UIApplication.didEnterBackgroundNotification) { surface, _ in
      surface.leaveScreen(background: true)
    }
    on(UIApplication.protectedDataWillBecomeUnavailableNotification) { surface, _ in
      surface.leaveScreen(background: true)
    }
    on(UIApplication.didReceiveMemoryWarningNotification) { surface, _ in
      surface.memoryWarning()
    }
  }

  /// A scene's notification is this surface's when it's the scene showing
  /// it (or the surface isn't on screen at all).
  private func isMine(_ object: Any?) -> Bool {
    guard let scene = root.window?.windowScene else { return true }
    return object as AnyObject? === scene
  }

  /// The stroke in progress ends, and the last change goes to Dart now
  /// rather than after the debounce. In the background, some time is asked
  /// for so Dart can save it before the app is suspended.
  func leaveScreen(background: Bool = false) {
    interrupt("background")
    let pending = Set(layers.filter { $0.value.dirty }.keys)
    flush()
    guard background, !pending.isEmpty else { return }
    awaitingSave.formUnion(pending)
    guard backgroundTask == .invalid else { return }
    backgroundTask = UIApplication.shared.beginBackgroundTask(withName: "ink-save") {
      [weak self] in self?.endBackgroundTask()
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 20) { [weak self] in
      self?.endBackgroundTask()
    }
  }

  private func endBackgroundTask() {
    awaitingSave.removeAll()
    guard backgroundTask != .invalid else { return }
    UIApplication.shared.endBackgroundTask(backgroundTask)
    backgroundTask = .invalid
  }

  /// Memory is short: caches go (the recognizers; PDFKit and PencilKit
  /// purge their own tiles), never ink. A pending change goes to Dart
  /// first, in case the app is ended next.
  func memoryWarning() {
    flush()
    recognizers.removeAll()
  }

  func pageJSON(_ page: Int) -> Any { page == 0 ? NSNull() : page }

  // MARK: Sending changes

  /// Coalesces: one drawingChanged per page after a short idle, never
  /// while a tool is down.
  func scheduleFlush() {
    flushWork?.cancel()
    let work = DispatchWorkItem { [weak self] in self?.flush() }
    flushWork = work
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.25, execute: work)
  }

  func flush() {
    flushWork?.cancel()
    flushWork = nil
    for page in layers.keys.sorted() where layers[page]?.dirty == true {
      guard var layer = layers[page] else { continue }
      localSeq += 1
      let event = changeEvent(page: page, layer: layer, seq: localSeq)
      layer.dirty = false
      layer.cause = nil
      layer.sentStrokes = layer.drawing.strokes.count
      layer.sentSeq = localSeq
      layers[page] = layer
      send("drawingChanged", event)
    }
  }

  func changeEvent(page: Int, layer: Layer, seq: Int) -> [String: Any] {
    let drawing = layer.drawing
    let data = drawing.dataRepresentation()
    let size = drawingSize(page: page)
    let all = drawing.strokes.isEmpty ? CGRect.zero : drawing.bounds
    var added = CGRect.null
    if drawing.strokes.count > layer.sentStrokes {
      for stroke in drawing.strokes[layer.sentStrokes...] { added = added.union(stroke.renderBounds) }
    }
    return [
      "page_1based": pageJSON(page),
      "ink_layer_id": layer.inkLayerId.inkJSON,
      "base_revision": layer.inkLayerId == nil ? NSNull() : layer.revision.inkJSON,
      "local_seq": seq,
      "cause": layer.cause ?? "stroke",
      "stroke_count": drawing.strokes.count,
      "bounds": InkRegion.json(InkRegion.normalized(all, in: size)),
      "new_bounds": added.isNull
        ? NSNull() : InkRegion.json(InkRegion.normalized(added, in: size)) as Any,
      "drawing": [
        "format": "pencilkit", "content_version": drawing.inkContentVersion,
        "bytes_base64": data.base64EncodedString(), "sha256": InkHash.sha256(data),
      ],
      "source_sha256": sourceSha256.inkJSON,
    ]
  }
}

/// A PDF surface below iOS 16: nothing to draw on, and Dart told why.
final class UnavailableSurface: InkSurface {
  init(frame: CGRect, params: InkParams, channel: InkEvents, error fields: [String: Any]) {
    super.init(frame: frame, params: params, channel: channel)
    error(fields)
  }
}

// MARK: - Handwriting (ipad.md §2)

/// Handwriting assist: a candidate text for some of a page's strokes, read
/// on the device by PKStrokeRecognizer (iPadOS 27+; no cloud fallback).
/// Native never applies or sends it: Dart offers it, editable. Each answer
/// names the drawing it read; one whose drawing changed meanwhile is stale
/// and carries no text.
extension InkSurface {
  static let handwritingMinOS = "27.0"

  /// `ready`'s `handwriting`.
  var handwritingJSON: [String: Any] {
    if #available(iOS 27.0, *) {
      return [
        "available": true, "min_os": Self.handwritingMinOS,
        "recognition_version": PKStrokeRecognizer.recognitionVersion,
        "supported_languages": Set(PKStrokeRecognizer.supportedLanguages.map(Self.bcp47)).sorted(),
      ]
    }
    return ["available": false, "min_os": Self.handwritingMinOS]
  }

  /// "en-US", "zh-Hant-TW": the script only where it isn't the language's
  /// usual one.
  @available(iOS 16.0, *)
  static func bcp47(_ language: Locale.Language) -> String {
    guard let code = language.languageCode?.identifier else { return language.minimalIdentifier }
    var parts = [code]
    if let script = language.script,
      Locale.Language(identifier: code).maximalIdentifier.split(separator: "-").dropFirst().first
        != Substring(script.identifier)
    {
      parts.append(script.identifier)
    }
    if let region = language.region { parts.append(region.identifier) }
    return parts.joined(separator: "-")
  }

  func recognizeText(_ args: [String: Any], _ result: @escaping FlutterResult) {
    let page = args["page_1based"] as? Int ?? 0
    guard #available(iOS 27.0, *) else {
      return result([
        "available": false, "reason": "os_too_old", "min_os": Self.handwritingMinOS,
        "os_version": UIDevice.current.systemVersion, "page_1based": pageJSON(page),
        "text": NSNull(),
      ])
    }
    let scope = args["scope"] as? String ?? "all"
    let region = InkRegion.rect(args["region"])
    guard ["selection", "region", "all"].contains(scope), scope != "region" || region != nil else {
      return result(FlutterError(code: "bad_request", message: "scope", details: nil))
    }
    // What's read is a drawing Dart already has.
    flush()
    let layer = layers[page] ?? Layer()
    let drawing = layer.drawing
    let ids: Set<UUID>
    switch scope {
    case "selection":
      let selected = liveCanvases.first { $0.tag == page }?.selection ?? []
      ids = Set(drawing.strokes.map(\.id)).intersection(selected)
    case "region":
      let size = drawingSize(page: page)
      let r = region ?? .zero
      let rect = CGRect(
        x: r.minX * size.width, y: r.minY * size.height, width: r.width * size.width,
        height: r.height * size.height)
      ids = Set(drawing.strokes.filter { $0.renderBounds.intersects(rect) }.map(\.id))
    default:
      ids = Set(drawing.strokes.map(\.id))
    }
    let wanted = (args["languages"] as? [String])?.map { Locale.Language(identifier: $0) }
    let key = "\(page)|" + (wanted?.map(Self.bcp47).joined(separator: ",") ?? "")
    let recognizer =
      recognizers[key] as? PKStrokeRecognizer
      ?? PKStrokeRecognizer(preferredLanguages: wanted?.isEmpty == false ? wanted : nil)
    recognizers[key] = recognizer
    let base: [String: Any] = [
      "available": true, "stale": false, "page_1based": pageJSON(page),
      "ink_layer_id": layer.inkLayerId.inkJSON,
      "base_revision": layer.inkLayerId == nil ? NSNull() : layer.revision.inkJSON,
      "local_seq": layer.sentSeq.inkJSON,
      "drawing_sha256": InkHash.sha256(drawing.dataRepresentation()),
      "stroke_count": ids.count, "text": NSNull(),
      "recognition_version": PKStrokeRecognizer.recognitionVersion,
    ]
    Task { @MainActor [weak self] in
      var answer = base
      var text: String?
      if !ids.isEmpty {
        await recognizer.updateDrawing(drawing)
        text = await recognizer.recognizedText(strokeIDs: ids)
      }
      let languages = await recognizer.languages.map(Self.bcp47)
      answer["languages"] = languages
      // The page's drawing changed while it was read: no text for it.
      guard let self, self.layers[page]?.drawing ?? PKDrawing() == drawing else {
        answer["stale"] = true
        return result(answer)
      }
      let trimmed = text?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
      answer["text"] = trimmed.isEmpty ? NSNull() : trimmed
      result(answer)
    }
  }

  /// The lasso's selection on a page, so Dart can offer to read it
  /// (iPadOS 27+: the only systems that call it).
  func canvasViewSelectionDidChange(_ canvasView: PKCanvasView) {
    guard #available(iOS 27.0, *) else { return }
    let page = canvasView.tag
    let selected = canvasView.selection
    let strokes = canvasView.drawing.strokes.filter { selected.contains($0.id) }
    let bounds = strokes.reduce(CGRect.null) { $0.union($1.renderBounds) }
    send(
      "strokeSelectionChanged",
      [
        "page_1based": pageJSON(page), "stroke_count": strokes.count,
        "bounds": bounds.isNull
          ? NSNull() : InkRegion.json(InkRegion.normalized(bounds, in: drawingSize(page: page))) as Any,
      ])
  }
}
