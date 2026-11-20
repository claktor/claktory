import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../annotations.dart';
import '../controls.dart';
import '../identity.dart';
import '../pane_chrome.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../resources.dart' show formatBytes;
import 'registry.dart';

/// An image: fitted to the pane (never enlarged past its pixels), over a
/// checkerboard where it's transparent, with its pixel size. Zoom with a
/// pinch, ⌘-scroll, ⌘+ ⌘− ⌘0 once clicked, or the header's controls; a
/// double click goes to 100% about the pointer and back to fitted; drag or
/// scroll to move. Comments are on regions of it: the header's Comment,
/// then a drag across it, marks one; each comment's region is outlined in
/// its agent's color, and a click on one picks it.
class ImageViewer extends StatefulWidget {
  const ImageViewer({super.key, required this.path});

  final String path;

  @override
  State<ImageViewer> createState() => _ImageViewerState();
}

class _ImageViewerState extends State<ImageViewer>
    with FileLoading<ImageViewer> {
  @override
  String get path => widget.path;

  final _zoom = TransformationController();

  /// The zoom the header shows: it follows [_zoom], but never while the
  /// pane lays out (fitting it then mustn't rebuild the header mid-build).
  final _shownScale = ValueNotifier(1.0);

  @override
  void initState() {
    super.initState();
    _zoom.addListener(_zoomChanged);
  }

  CommentsController? _wired;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final comments = this.comments;
    if (comments != _wired) {
      _wired = comments;
      comments
        ?..yOfRegion = _yOfRegion
        ..revealRegion = _revealRegion;
    }
  }

  /// Dragging across the image marks a region to comment on, rather than
  /// moving it.
  var _marking = false;

  /// The drag marking a region, as fractions of the image.
  Offset? _dragFrom;
  Offset? _dragTo;

  /// [local] (in the pane) as a fraction of the image, kept inside it.
  Offset _fraction(Offset local) {
    final image = _image!;
    final inverse = Matrix4.inverted(_zoom.value);
    final at = MatrixUtils.transformPoint(inverse, local);
    return Offset(
      (at.dx / image.width).clamp(0.0, 1.0),
      (at.dy / image.height).clamp(0.0, 1.0),
    );
  }

  /// [region] (fractions of the image) where it shows in the pane.
  Rect _onScreen(Rect region) {
    final image = _image!;
    return MatrixUtils.transformRect(
      _zoom.value,
      Rect.fromLTWH(
        region.left * image.width,
        region.top * image.height,
        region.width * image.width,
        region.height * image.height,
      ),
    );
  }

  double? _yOfRegion(Rect region) {
    if (_image == null || _pane.isEmpty) return null;
    return _onScreen(region).top.clamp(0.0, _pane.height);
  }

  /// Moves the image so [region] is in the middle of the pane, if any of
  /// it is out of sight.
  void _revealRegion(Rect region) {
    if (_image == null || _pane.isEmpty) return;
    final shown = _onScreen(region);
    if ((Offset.zero & _pane).contains(shown.topLeft) &&
        (Offset.zero & _pane).contains(shown.bottomRight)) {
      return;
    }
    final by = _pane.center(Offset.zero) - shown.center;
    setState(() {
      _fitted = false;
      _zoom.value = Matrix4.translationValues(by.dx, by.dy, 0)
        ..multiply(_zoom.value);
    });
  }

  void _toggleMarking() {
    setState(() {
      _marking = !_marking;
      _dragFrom = _dragTo = null;
    });
    if (!_marking && comments?.selection.value is Region) {
      comments?.selection.value = null;
    }
  }

  void _markStart(DragStartDetails details) {
    setState(() {
      _dragFrom = _fraction(details.localPosition);
      _dragTo = _dragFrom;
    });
  }

  void _markUpdate(DragUpdateDetails details) {
    setState(() => _dragTo = _fraction(details.localPosition));
  }

  void _markEnd(DragEndDetails details) {
    final from = _dragFrom;
    final to = _dragTo;
    setState(() {
      _dragFrom = _dragTo = null;
      _marking = false;
    });
    if (from == null || to == null) return;
    final region = Region(Rect.fromPoints(from, to));
    if (region.isEmpty) return;
    final comments = this.comments;
    if (comments == null) return;
    comments.selection.value = region;
    // On a narrow pane, straight to writing it: there's no margin to
    // offer Comment in.
    if (comments.narrow.value) comments.comment();
  }

  /// A click on a comment's region picks it; anywhere else lets go.
  void _tapped(TapUpDetails details) {
    final comments = this.comments;
    if (comments == null || _image == null) return;
    final at = _fraction(details.localPosition);
    String? hit;
    var smallest = double.infinity;
    for (final thread in comments.shown) {
      final region = comments.regions[thread.id];
      if (region == null || !region.contains(at)) continue;
      final area = region.width * region.height;
      if (area < smallest) {
        smallest = area;
        hit = thread.id;
      }
    }
    comments.focus(hit);
    if (comments.selection.value is Region && comments.composing == null) {
      comments.selection.value = null;
    }
  }

  void _zoomChanged() {
    final phase = SchedulerBinding.instance.schedulerPhase;
    // The margin's cards follow the regions too.
    void follow() {
      _shownScale.value = _scale;
      comments?.relayout();
    }

    if (phase == SchedulerPhase.idle ||
        phase == SchedulerPhase.postFrameCallbacks) {
      follow();
    } else {
      SchedulerBinding.instance.addPostFrameCallback((_) {
        if (mounted) follow();
      });
    }
  }

  ui.Image? _image;
  String? _undecodable;

  /// The scale that fits the image in the pane (at most 1), and the pane's
  /// size, from the last layout.
  double _fit = 1;
  Size _pane = Size.zero;

  /// Whether the view is still fitted, so it refits as the pane resizes.
  var _fitted = true;

  final _focus = FocusNode(debugLabel: 'image');

  /// Where the last double click was, in the pane.
  var _doubleTapAt = Offset.zero;

  @override
  void loaded(FileContent file) {
    _undecodable = null;
    if (file.truncated) {
      // Only its start came: it can't be decoded, so don't try.
      _image?.dispose();
      _image = null;
      _undecodable =
          'This image is too big to open here (${formatBytes(file.size)}).';
      return;
    }
    _decode(Uint8List.fromList(file.data));
  }

  Future<void> _decode(Uint8List bytes) async {
    try {
      final codec = await ui.instantiateImageCodec(bytes);
      final frame = await codec.getNextFrame();
      if (!mounted) return;
      setState(() {
        _image?.dispose();
        _image = frame.image;
        _fitted = true;
      });
      _showComments();
    } on Exception {
      if (mounted) {
        setState(() => _undecodable = "Claktory can't open this image.");
      }
    }
  }

  /// Comments are on whole images Claktory read: a cut one has no
  /// version to belong to.
  bool get _commentable {
    final file = this.file;
    return file != null &&
        _image != null &&
        file.error.isEmpty &&
        !file.truncated &&
        file.sha256.isNotEmpty;
  }

  void _showComments() {
    final comments = this.comments;
    if (comments == null || !_commentable) return;
    comments
      ..image = _image
      ..show(kind: 'image', text: '', version: file!.sha256);
  }

  @override
  void dispose() {
    _focus.dispose();
    _shownScale.dispose();
    _zoom.dispose();
    _image?.dispose();
    super.dispose();
  }

  double get _scale => _zoom.value.getMaxScaleOnAxis();

  /// Zooms to [scale] (of the image's own size) keeping the point [at] in
  /// the pane where it is: the pane's middle unless given.
  void _zoomTo(double scale, {Offset? at}) {
    if (_image == null) return;
    final focal = at ?? _pane.center(Offset.zero);
    final clamped = scale.clamp(_minScale, 8.0);
    final factor = clamped / _scale;
    setState(() {
      _fitted = false;
      _zoom.value = Matrix4.identity()
        ..translateByDouble(focal.dx, focal.dy, 0, 1)
        ..scaleByDouble(factor, factor, factor, 1)
        ..translateByDouble(-focal.dx, -focal.dy, 0, 1)
        ..multiply(_zoom.value);
    });
  }

  /// A tenth of its size, or less if that's what fits.
  double get _minScale => _fit < 0.1 ? _fit : 0.1;

  /// Double click: 100% about the pointer, or back to fitted from there.
  void _toggleActualSize() {
    if ((_scale - 1).abs() < 0.01 && _fit < 1) {
      _fitNow();
    } else {
      _zoomTo(1, at: _doubleTapAt);
    }
  }

  void _zoomIn() => _zoomTo(_scale * 1.25);
  void _zoomOut() => _zoomTo(_scale / 1.25);

  void _fitNow() {
    setState(() => _fitted = true);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final file = this.file;
    final image = _image;
    final problem =
        waiting() ??
        (_undecodable == null
            ? null
            : ViewerProblem(_undecodable!, path: path));
    final details = this.details(
      [
        if (image != null) '${image.width} × ${image.height}',
        if (file != null && file.error.isEmpty) formatBytes(file.size),
      ].join(' · '),
    );
    final comments = _commentable ? this.comments : null;
    Widget glyphButton(String label, String icon, VoidCallback onTap) =>
        GlyphButton(
          label: label,
          active: false,
          onTap: onTap,
          glyph: (color) =>
              ExtensionGlyph(icon: icon, name: label, color: color),
        );
    final controls = <Widget>[
      if (comments != null) ...[
        CommentsButton(controller: comments),
        HeaderTextButton('Comment', _toggleMarking, active: _marking),
      ],
      if (image != null) ...[
        ValueListenableBuilder(
          valueListenable: _shownScale,
          builder: (context, scale, _) => Padding(
            padding: const EdgeInsets.symmetric(horizontal: Space.xs),
            child: Text(
              '${(scale * 100).round()}%',
              style: TextStyles.caption.copyWith(
                color: palette.textMuted,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
          ),
        ),
        glyphButton('Zoom out', 'minus', _zoomOut),
        glyphButton('Zoom in', 'plus', _zoomIn),
        HeaderTextButton('Fit', _fitNow),
        HeaderTextButton('100%', () => _zoomTo(1)),
      ],
      reloadButton(),
    ];
    final Widget body;
    if (problem != null || image == null) {
      body = problem ?? const SizedBox.shrink();
    } else {
      body = LayoutBuilder(
        builder: (context, constraints) {
          _pane = constraints.biggest;
          final fit = [
            1.0,
            (_pane.width - 2 * Space.l) / image.width,
            (_pane.height - 2 * Space.l) / image.height,
          ].reduce((a, b) => a < b ? a : b).clamp(0.01, 1.0);
          _fit = fit;
          // Scaled on every axis, as InteractiveViewer scales: its zoom
          // limits read the largest.
          if (_fitted) {
            final width = image.width * fit;
            final height = image.height * fit;
            _zoom.value = Matrix4.identity()
              ..translateByDouble(
                (_pane.width - width) / 2,
                (_pane.height - height) / 2,
                0,
                1,
              )
              ..scaleByDouble(fit, fit, fit, 1);
          }
          return CallbackShortcuts(
            bindings: {
              const SingleActivator(LogicalKeyboardKey.equal, meta: true):
                  _zoomIn,
              const SingleActivator(
                LogicalKeyboardKey.equal,
                meta: true,
                shift: true,
              ): _zoomIn,
              const SingleActivator(LogicalKeyboardKey.add, meta: true):
                  _zoomIn,
              const SingleActivator(LogicalKeyboardKey.minus, meta: true):
                  _zoomOut,
              const SingleActivator(LogicalKeyboardKey.digit0, meta: true):
                  _fitNow,
              const SingleActivator(LogicalKeyboardKey.escape): () {
                if (_marking) _toggleMarking();
              },
            },
            child: Focus(
              focusNode: _focus,
              child: GestureDetector(
                onTapUp: comments == null ? null : _tapped,
                onDoubleTapDown: (details) =>
                    _doubleTapAt = details.localPosition,
                onDoubleTap: _toggleActualSize,
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
                  child: Stack(
                    children: [
                      InteractiveViewer(
                        transformationController: _zoom,
                        constrained: false,
                        minScale: _minScale,
                        maxScale: 8,
                        boundaryMargin: const EdgeInsets.all(double.infinity),
                        onInteractionStart: (_) => _fitted = false,
                        child: SizedBox(
                          width: image.width.toDouble(),
                          height: image.height.toDouble(),
                          child: CustomPaint(
                            painter: _Checkerboard(
                              palette.hairline,
                              zoom: _zoom,
                            ),
                            child: RawImage(
                              image: image,
                              filterQuality: _scale > 2
                                  ? FilterQuality.none
                                  : FilterQuality.medium,
                            ),
                          ),
                        ),
                      ),
                      if (comments != null)
                        Positioned.fill(
                          child: IgnorePointer(
                            child: CustomPaint(
                              painter: _Regions(
                                comments: comments,
                                zoom: _zoom,
                                onScreen: _onScreen,
                                drag: _dragFrom == null || _dragTo == null
                                    ? null
                                    : Rect.fromPoints(_dragFrom!, _dragTo!),
                                signal: palette.signal,
                                colorOf: (agentId) => agentColor(
                                  comments.backend.workspace.workspace.agents
                                          .where((a) => a.id == agentId)
                                          .firstOrNull
                                          ?.color ??
                                      '',
                                ),
                              ),
                            ),
                          ),
                        ),
                      // While marking, a drag draws a region instead of
                      // moving the image.
                      if (_marking)
                        Positioned.fill(
                          child: MouseRegion(
                            cursor: SystemMouseCursors.precise,
                            child: GestureDetector(
                              behavior: HitTestBehavior.opaque,
                              // From where the pointer went down, not
                              // where the drag was recognised.
                              dragStartBehavior: DragStartBehavior.down,
                              onPanStart: _markStart,
                              onPanUpdate: _markUpdate,
                              onPanEnd: _markEnd,
                            ),
                          ),
                        ),
                    ],
                  ),
                ),
              ),
            ),
          );
        },
      );
    }
    return PaneScaffold(
      details: details,
      controls: controls,
      body: ClipRect(
        child: ColoredBox(
          color: palette.canvas,
          child: comments == null
              ? body
              : Commented(controller: comments, child: body),
        ),
      ),
    );
  }
}

/// Each comment's region outlined in its agent's color, the picked one
/// stronger, and the person's selection and the region being dragged in
/// signal, in the pane's coordinates (so lines stay thin at any zoom).
class _Regions extends CustomPainter {
  _Regions({
    required this.comments,
    required this.zoom,
    required this.onScreen,
    required this.drag,
    required this.signal,
    required this.colorOf,
  }) : super(repaint: Listenable.merge([comments, comments.selection, zoom]));

  final CommentsController comments;
  final TransformationController zoom;
  final Rect Function(Rect region) onScreen;
  final Rect? drag;
  final Color signal;
  final Color Function(String agentId) colorOf;

  @override
  void paint(Canvas canvas, Size size) {
    void box(Rect region, Color color, {required bool strong}) {
      final rect = onScreen(region);
      canvas
        ..drawRect(
          rect,
          Paint()..color = color.withValues(alpha: strong ? 0.20 : 0.10),
        )
        ..drawRect(
          rect,
          Paint()
            ..color = color
            ..style = PaintingStyle.stroke
            ..strokeWidth = strong ? 2.5 : 1.5,
        );
    }

    for (final thread in comments.shown) {
      final region = comments.regions[thread.id];
      if (region == null) continue;
      box(
        region,
        colorOf(thread.agentId),
        strong: comments.focused == thread.id,
      );
    }
    final selected = comments.selection.value;
    if (selected is Region) box(selected.rect, signal, strong: false);
    final drag = this.drag;
    if (drag != null) box(drag, signal, strong: false);
  }

  @override
  bool shouldRepaint(_Regions old) =>
      old.drag != drag || old.signal != signal || old.comments != comments;
}

/// Faint squares under an image, so where it's transparent shows: 8
/// points on screen each, whatever the [zoom].
class _Checkerboard extends CustomPainter {
  _Checkerboard(this.color, {required this.zoom}) : super(repaint: zoom);

  final Color color;
  final TransformationController zoom;

  @override
  void paint(Canvas canvas, Size size) {
    final cell = 8 / zoom.value.getMaxScaleOnAxis().clamp(0.01, 8.0);
    final paint = Paint()..color = color;
    for (var row = 0; row * cell < size.height; row++) {
      for (var col = row.isEven ? 0 : 1; col * cell < size.width; col += 2) {
        canvas.drawRect(
          Rect.fromLTWH(
            col * cell,
            row * cell,
            cell,
            cell,
          ).intersect(Offset.zero & size),
          paint,
        );
      }
    }
  }

  @override
  bool shouldRepaint(_Checkerboard old) =>
      old.color != color || old.zoom != zoom;
}
