// What every document surface shares (InkView, PaperView): the channel, the
// ink layers it holds per page, coalesced drawingChanged events, its own
// undo, tools and the Pencil's double-tap, and interruptions.

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
  private var attached = false
  private var queued: [(String, Any?)] = []
  var tool: PKTool = PKInkingTool(.pen, color: .black, width: 3)
  private var toolState: [String: Any] = ["tool": "pen", "color": "#000000FF", "width": 3.0]
  private var previousTool: (PKTool, [String: Any])?
  var drawingPolicy: PKCanvasViewDrawingPolicy

  init(frame: CGRect, params: InkParams, channel: InkEvents) {
    self.params = params
    self.channel = channel
    drawingPolicy = params.drawingPolicy
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
    let pencil = UIPencilInteraction()
    pencil.delegate = self
    root.addInteraction(pencil)
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else { return result(nil) }
      self.handle(call, result)
    }
  }

  deinit {
    flushWork?.cancel()
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
      let name = args["drawing_policy"] as? String
      drawingPolicy =
        name == "pencil_only" ? .pencilOnly : name == "any_input" ? .anyInput : .default
      for canvas in liveCanvases { canvas.drawingPolicy = drawingPolicy }
      result(["drawing_policy": name ?? "default"])
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
    default:
      result(FlutterMethodNotImplemented)
    }
  }

  // MARK: Subclasses

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

  // MARK: Loading

  private func load(_ refs: [[String: Any]]) -> [String: Any] {
    var loaded: [[String: Any]] = []
    var unreadable: [[String: Any]] = []
    for ref in refs {
      let page = ref["page_1based"] as? Int ?? 0
      let wire = ref["drawing"] as? [String: Any] ?? [:]
      var entry: [String: Any] = [
        "page_1based": ref["page_1based"] ?? NSNull(),
        "ink_layer_id": ref["ink_layer_id"] ?? NSNull(), "revision": ref["revision"] ?? NSNull(),
      ]
      let version = wire["content_version"] as? Int ?? 1
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
      layer.inkLayerId = ref["ink_layer_id"] as? String
      layer.revision = ref["revision"] as? Int
      layer.drawing = drawing
      layer.sentStrokes = drawing.strokes.count
      layer.dirty = false
      layers[page] = layer
      loading += 1
      show(drawing, page: page)
      loading -= 1
      entry["strokes"] = drawing.strokes.count
      loaded.append(entry)
    }
    // A loaded drawing isn't something to undo.
    undo.removeAllActions()
    send("undoState", undoState)
    return ["loaded": loaded, "unreadable": unreadable]
  }

  /// A new canvas for a page (or the surface's only one), with this
  /// surface's tool, policy, undo and drawing.
  func makeCanvas(page: Int) -> InkCanvas {
    let canvas = InkCanvas()
    canvas.surfaceUndo = undo
    canvas.backgroundColor = .clear
    canvas.isOpaque = false
    canvas.drawingPolicy = drawingPolicy
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

  /// Apple Pencil's double-tap: what the person chose in Settings, for the
  /// non-destructive tools only (never send, accept or delete: §2).
  func pencilInteractionDidTap(_ interaction: UIPencilInteraction) {
    switch UIPencilInteraction.preferredTapAction {
    case .switchEraser:
      if toolState["tool"] as? String == "eraser", let previous = previousTool {
        swap(to: previous)
      } else {
        previousTool = (tool, toolState)
        tool = Self.tool("eraser", color: .black, width: 12)
        toolState = ["tool": "eraser", "color": NSNull(), "width": 12.0]
        applyTool(source: "pencil_double_tap")
      }
    case .switchPrevious:
      if let previous = previousTool { swap(to: previous) }
    default:
      break
    }
  }

  private func swap(to previous: (PKTool, [String: Any])) {
    let current = (tool, toolState)
    (tool, toolState) = previous
    previousTool = current
    applyTool(source: "pencil_double_tap")
  }

  private func applyTool(source: String) {
    for canvas in liveCanvases { canvas.tool = tool }
    var event = toolState
    event["source"] = source
    send("toolChanged", event)
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
    activePage = canvasView.tag
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
    if activePage == nil { scheduleFlush() }
  }

  /// A layout change, rotation, undo or hiding while a tool is down: the
  /// stroke can't be kept (PencilKit drops it), so Dart hears so, and the
  /// stroke's end comes first.
  func interrupt(_ reason: String) {
    guard let page = activePage else { return }
    activePage = nil
    send("strokeActive", ["page_1based": pageJSON(page), "active": false])
    send("strokeInterrupted", ["page_1based": pageJSON(page), "reason": reason, "kept": false])
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
      "source_sha256": params.sourceSha256.inkJSON,
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
