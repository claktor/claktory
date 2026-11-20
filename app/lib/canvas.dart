import 'package:flutter/gestures.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'identity.dart';

/// Zooms and pans a [ZoomCanvas] from outside it: a pane header's buttons,
/// say. Its [scale] is what the canvas shows now.
class CanvasController extends ChangeNotifier {
  _ZoomCanvasState? _canvas;

  /// The content's scale on screen: 1 is its own size.
  double get scale => _canvas?._scale ?? 1;

  /// Whether the whole content is fitted to the view (as it opens, and
  /// after [fit]), so it fits again as the view changes size.
  bool get fitted => _canvas?._fitted ?? true;

  void fit() => _canvas?._fit();
  void zoomIn() => _canvas?._zoomBy(1.25);
  void zoomOut() => _canvas?._zoomBy(1 / 1.25);

  /// Zooms to [scale], keeping [at] (in the view; its middle unless given)
  /// where it is.
  void zoomTo(double scale, {Offset? at}) => _canvas?._zoomTo(scale, at: at);

  /// Brings [rect] (in the content's coordinates) into view, as large as
  /// fits up to its own size.
  void show(Rect rect) => _canvas?._show(rect);

  /// Where [rect] (in the content's coordinates) is in the view now.
  Rect toView(Rect rect) {
    final canvas = _canvas;
    if (canvas == null) return rect;
    return MatrixUtils.transformRect(canvas._transform.value, rect);
  }

  void _changed() => notifyListeners();
}

/// A surface of [size] that pans and zooms: drag or scroll (two fingers on
/// a trackpad) to move it, pinch or ⌘-scroll to zoom about the pointer,
/// double-click to zoom in there, and once clicked ⌘+, ⌘− and ⌘0 (fit).
/// It opens fitted to the view, with [padding] around, and stays fitted as
/// the view changes size until it's moved.
///
/// [child] is laid out at [size], in content coordinates; anything it hit
/// tests (a click on a node) works at every zoom.
class ZoomCanvas extends StatefulWidget {
  const ZoomCanvas({
    super.key,
    required this.size,
    required this.child,
    this.controller,
    this.padding = const EdgeInsets.all(Space.xl),
    this.maxScale = 4,
    this.background,
  });

  final Size size;
  final Widget child;
  final CanvasController? controller;
  final EdgeInsets padding;
  final double maxScale;

  /// Painted behind the content, filling the view at every zoom (a grid,
  /// say); it gets the canvas's current transform.
  final CustomPainter Function(Matrix4 transform)? background;

  @override
  State<ZoomCanvas> createState() => _ZoomCanvasState();
}

class _ZoomCanvasState extends State<ZoomCanvas> {
  final _transform = TransformationController();
  final _focus = FocusNode(debugLabel: 'canvas');
  var _view = Size.zero;
  var _fitted = true;
  var _doubleTapAt = Offset.zero;

  @override
  void initState() {
    super.initState();
    widget.controller?._canvas = this;
    _transform.addListener(_moved);
  }

  @override
  void didUpdateWidget(ZoomCanvas old) {
    super.didUpdateWidget(old);
    if (old.controller != widget.controller) {
      old.controller?._canvas = null;
      widget.controller?._canvas = this;
    }
    if (old.size != widget.size && _fitted) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted && _fitted) _fit();
      });
    }
  }

  @override
  void dispose() {
    if (widget.controller?._canvas == this) widget.controller?._canvas = null;
    _transform.removeListener(_moved);
    _transform.dispose();
    _focus.dispose();
    super.dispose();
  }

  void _moved() {
    // Fitting to a new view size happens while laying out: tell listeners
    // (a zoom readout) after the frame.
    if (SchedulerBinding.instance.schedulerPhase ==
        SchedulerPhase.persistentCallbacks) {
      SchedulerBinding.instance.addPostFrameCallback((_) {
        if (mounted) widget.controller?._changed();
      });
      return;
    }
    widget.controller?._changed();
  }

  double get _scale => _transform.value.getMaxScaleOnAxis();

  /// The scale that fits the whole content in the view, at most 1.
  double get _fitScale {
    final room = widget.padding.deflateSize(_view);
    if (room.isEmpty || widget.size.isEmpty) return 1;
    final scale = [
      1.0,
      room.width / widget.size.width,
      room.height / widget.size.height,
    ].reduce((a, b) => a < b ? a : b);
    return scale <= 0 ? 1 : scale;
  }

  /// Down to a fifth of the content's size, or less when that's what fits.
  double get _minScale => _fitScale < 0.2 ? _fitScale : 0.2;

  /// The transform that shows the content at [scale] with its middle in
  /// the view's middle.
  Matrix4 _centered(double scale) {
    final width = widget.size.width * scale;
    final height = widget.size.height * scale;
    // Scaled on every axis, as InteractiveViewer scales: its zoom limits
    // read the largest.
    return Matrix4.identity()
      ..translateByDouble(
        (_view.width - width) / 2,
        (_view.height - height) / 2,
        0,
        1,
      )
      ..scaleByDouble(scale, scale, scale, 1);
  }

  void _fit() {
    if (_view.isEmpty) return;
    _transform.value = _centered(_fitScale);
    if (!_fitted) setState(() => _fitted = true);
  }

  void _zoomBy(double factor) => _zoomTo(_scale * factor);

  void _zoomTo(double scale, {Offset? at}) {
    if (_view.isEmpty) return;
    final focal = at ?? _view.center(Offset.zero);
    final clamped = scale.clamp(_minScale, widget.maxScale);
    final factor = clamped / _scale;
    _transform.value = Matrix4.identity()
      ..translateByDouble(focal.dx, focal.dy, 0, 1)
      ..scaleByDouble(factor, factor, factor, 1)
      ..translateByDouble(-focal.dx, -focal.dy, 0, 1)
      ..multiply(_transform.value);
    if (_fitted) setState(() => _fitted = false);
  }

  void _show(Rect rect) {
    if (_view.isEmpty || rect.isEmpty) return;
    final room = widget.padding.deflateSize(_view);
    final scale = [
      1.0,
      room.width / rect.width,
      room.height / rect.height,
    ].reduce((a, b) => a < b ? a : b).clamp(_minScale, widget.maxScale);
    final offset = _view.center(Offset.zero) - rect.center * scale;
    _transform.value = Matrix4.identity()
      ..translateByDouble(offset.dx, offset.dy, 0, 1)
      ..scaleByDouble(scale, scale, scale, 1);
    if (_fitted) setState(() => _fitted = false);
  }

  /// Double click: twice as close about the pointer, or back to fitted when
  /// already at the closest.
  void _doubleTap() {
    if (_scale >= widget.maxScale - 0.01) return _fit();
    _zoomTo(_scale * 2, at: _doubleTapAt);
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final view = constraints.biggest;
        if (view != _view) {
          final first = _view.isEmpty;
          _view = view;
          if (first || _fitted) _transform.value = _centered(_fitScale);
        }
        Widget content = InteractiveViewer(
          transformationController: _transform,
          constrained: false,
          minScale: _minScale,
          maxScale: widget.maxScale,
          boundaryMargin: const EdgeInsets.all(double.infinity),
          onInteractionStart: (_) {
            if (_fitted) setState(() => _fitted = false);
          },
          child: SizedBox.fromSize(size: widget.size, child: widget.child),
        );
        final background = widget.background;
        if (background != null) {
          content = Stack(
            fit: StackFit.expand,
            children: [
              RepaintBoundary(
                child: ValueListenableBuilder(
                  valueListenable: _transform,
                  builder: (context, transform, _) =>
                      CustomPaint(painter: background(transform)),
                ),
              ),
              content,
            ],
          );
        }
        return CallbackShortcuts(
          bindings: {
            const SingleActivator(LogicalKeyboardKey.equal, meta: true): () =>
                _zoomBy(1.25),
            const SingleActivator(
              LogicalKeyboardKey.equal,
              meta: true,
              shift: true,
            ): () =>
                _zoomBy(1.25),
            const SingleActivator(LogicalKeyboardKey.add, meta: true): () =>
                _zoomBy(1.25),
            const SingleActivator(LogicalKeyboardKey.minus, meta: true): () =>
                _zoomBy(1 / 1.25),
            const SingleActivator(LogicalKeyboardKey.digit0, meta: true): _fit,
          },
          child: Focus(
            focusNode: _focus,
            child: GestureDetector(
              onDoubleTapDown: (details) =>
                  _doubleTapAt = details.localPosition,
              onDoubleTap: _doubleTap,
              child: Listener(
                // A click takes the keys for zooming.
                onPointerDown: (_) => _focus.requestFocus(),
                // ⌘-scroll zooms about the pointer; a plain scroll pans,
                // and a pinch zooms, as InteractiveViewer does.
                onPointerSignal: (event) {
                  if (event is PointerScrollEvent &&
                      HardwareKeyboard.instance.isMetaPressed) {
                    _zoomTo(
                      _scale * (event.scrollDelta.dy < 0 ? 1.1 : 1 / 1.1),
                      at: event.localPosition,
                    );
                  }
                },
                child: ClipRect(child: content),
              ),
            ),
          ),
        );
      },
    );
  }
}
