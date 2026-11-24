import 'dart:async';
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../annotations.dart';
import '../controls.dart';
import '../identity.dart';
import '../ink_preview.dart';
import '../ipad/pencil_hint.dart';
import '../mobile/metrics.dart';
import '../pane_chrome.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../resources.dart' show formatBytes;
import '../review_answer.dart' show ReviewAnswering;
import '../paced.dart' show FilePreviews;
import 'preview_badge.dart';
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

  @override
  String get loadingLabel => 'Loading image…';

  @override
  bool get previews => true;

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
        ..revealRegion = _revealRegion
        ..rectOfRegion = _rectOfRegion;
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
    final image = _box!;
    final inverse = Matrix4.inverted(_zoom.value);
    final at = MatrixUtils.transformPoint(inverse, local);
    return Offset(
      (at.dx / image.width).clamp(0.0, 1.0),
      (at.dy / image.height).clamp(0.0, 1.0),
    );
  }

  /// [region] (fractions of the image) where it shows in the pane.
  Rect _onScreen(Rect region) {
    final image = _box!;
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

  /// The pane's box, for where a region is on screen.
  final _paneKey = GlobalKey();

  Rect? _rectOfRegion(Rect region) {
    final box = _paneKey.currentContext?.findRenderObject();
    if (_image == null || box is! RenderBox || !box.attached) return null;
    return _onScreen(region).shift(box.localToGlobal(Offset.zero));
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

  /// The pointer is over the image (for its Comment button).
  var _hovered = false;

  /// No pointer to hover with: the Comment button always shows.
  bool get _touch =>
      defaultTargetPlatform == TargetPlatform.iOS ||
      defaultTargetPlatform == TargetPlatform.android;

  /// A click while picking: a comment on that spot.
  void _markSpot(Offset at) {
    setState(() {
      _dragFrom = _dragTo = null;
      _marking = false;
    });
    _commentOn(Region(Rect.fromLTWH(at.dx, at.dy, 0, 0)));
  }

  void _markEnd(DragEndDetails details) => _markDone();

  /// On a phone, marking: one finger marks, as a drag does, and two zoom
  /// and move the image, so a small spot can be found first. The zoom as
  /// the pinch began, and where its fingers were.
  Matrix4? _pinchFrom;
  var _pinchAt = Offset.zero;

  /// Fingers have pinched since they all last lifted: nothing marks until
  /// they have. The fingers down, to know.
  var _pinched = false;
  final _fingers = <int>{};

  void _fingerUp(PointerEvent event) {
    _fingers.remove(event.pointer);
    if (_fingers.isEmpty) _pinched = false;
  }

  void _scaleStart(ScaleStartDetails details) {
    if (details.pointerCount > 1) {
      _pinched = true;
      setState(() {
        _dragFrom = _dragTo = null;
        _fitted = false;
      });
      _pinchFrom = _zoom.value.clone();
      _pinchAt = details.localFocalPoint;
      return;
    }
    if (_pinched) return;
    setState(() {
      _dragFrom = _fraction(details.localFocalPoint);
      _dragTo = _dragFrom;
    });
  }

  void _scaleUpdate(ScaleUpdateDetails details) {
    final from = _pinchFrom;
    if (from != null) {
      final was = from.getMaxScaleOnAxis();
      final factor = (was * details.scale).clamp(_minScale, 8.0) / was;
      final at = details.localFocalPoint;
      _zoom.value = Matrix4.identity()
        ..translateByDouble(at.dx, at.dy, 0, 1)
        ..scaleByDouble(factor, factor, factor, 1)
        ..translateByDouble(-_pinchAt.dx, -_pinchAt.dy, 0, 1)
        ..multiply(from);
      return;
    }
    if (_dragFrom == null) return;
    setState(() => _dragTo = _fraction(details.localFocalPoint));
  }

  /// Fingers were added or lifted (the pinch goes on, or one is left), or
  /// all lifted: only then does what a finger dragged across become the
  /// comment's area.
  void _scaleEnd(ScaleEndDetails details) {
    _pinchFrom = null;
    if (details.pointerCount > 0 || _dragFrom == null) return;
    _markDone();
  }

  /// The whole image, with no gesture to draw it.
  void _markWhole() {
    setState(() {
      _dragFrom = _dragTo = null;
      _marking = false;
    });
    _commentOn(const Region(Rect.fromLTWH(0, 0, 1, 1)));
  }

  void _markDone() {
    final from = _dragFrom;
    final to = _dragTo;
    setState(() {
      _dragFrom = _dragTo = null;
      _marking = false;
    });
    if (from == null || to == null) return;
    final area = Region(Rect.fromPoints(from, to));
    // Too small to be an area: the spot it started on.
    _commentOn(
      area.isEmpty ? Region(Rect.fromLTWH(from.dx, from.dy, 0, 0)) : area,
    );
  }

  void _commentOn(Region region) {
    final comments = this.comments;
    if (comments == null) return;
    comments.selection.value = region;
    // Comment was asked for already: straight to writing it.
    comments.comment();
  }

  /// A click on a comment's region picks it; anywhere else lets go.
  void _tapped(TapUpDetails details) {
    final comments = this.comments;
    if (comments == null || _image == null) return;
    final at = _fraction(details.localPosition);
    String? hit;
    // A pin first: a spot has nothing else to click. A fingertip's
    // reach on a phone: a 44pt target around it.
    var nearest = isPhoneLayout(context) ? Touch.target / 2 : 12.0;
    for (final thread in comments.shown) {
      final region = comments.regions[thread.id];
      if (region == null) continue;
      final away = (_onScreen(region).topLeft - details.localPosition).distance;
      if (away <= nearest) {
        nearest = away;
        hit = thread.id;
      }
    }
    var smallest = hit == null ? double.infinity : -1.0;
    for (final thread in comments.shown) {
      final region = comments.regions[thread.id];
      if (region == null || !region.contains(at)) continue;
      final area = region.width * region.height;
      if (area < smallest) {
        smallest = area;
        hit = thread.id;
      }
    }
    if (hit != null) {
      comments.focus(hit);
    } else {
      comments.letGo();
    }
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
      _sharpEnough();
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

  /// The image's box, in its own pixels: the original's, also while a
  /// preview of it shows (#379), so zoom, comments' regions and ink are
  /// where they'd be on the original, and nothing moves when it comes.
  Size? _box;

  /// The version last decoded: the original replacing its preview keeps
  /// the zoom.
  String _decoded = '';

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
    _decode(file, Uint8List.fromList(file.data));
  }

  Future<void> _decode(FileContent file, Uint8List bytes) async {
    try {
      final codec = await ui.instantiateImageCodec(bytes);
      final frame = await codec.getNextFrame();
      if (!mounted) return;
      final image = frame.image;
      setState(() {
        _image?.dispose();
        _image = image;
        _box =
            file.isPreview &&
                file.originalPixelWidth > 0 &&
                file.originalPixelHeight > 0
            ? Size(
                file.originalPixelWidth.toDouble(),
                file.originalPixelHeight.toDouble(),
              )
            : Size(image.width.toDouble(), image.height.toDouble());
        // The same version again (its original, say): as it was shown.
        if (file.sha256.isEmpty || file.sha256 != _decoded) _fitted = true;
        _decoded = file.sha256;
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

  /// Each of the bitmap's pixels on screen, in the screen's pixels: past 1
  /// it's enlarged.
  double get _pixelScale {
    final image = _image, box = _box;
    if (image == null || box == null || image.width == 0) return _scale;
    return _scale * box.width / image.width;
  }

  /// Zoomed in past a preview's sharpness: the original, by itself.
  void _sharpEnough() {
    final file = this.file;
    if (file == null || !file.isPreview || !mounted) return;
    if (originalLoad is! OriginalNotLoaded) return;
    final ratio = MediaQuery.maybeDevicePixelRatioOf(context) ?? 1;
    if (_pixelScale * ratio > 1.25) unawaited(loadOriginal());
  }

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
    final box = _box;
    final problem =
        waiting() ??
        (_undecodable == null
            ? null
            : ViewerProblem(_undecodable!, path: path));
    final details = this.details(
      [
        if (box != null) '${box.width.round()} × ${box.height.round()}',
        if (file != null && file.error.isEmpty)
          formatBytes(file.isPreview ? file.originalSize : file.size),
      ].join(' · '),
    );
    // Answering a review, nothing but the answer: no comments meanwhile.
    final comments = _commentable && !ReviewAnswering.of(context)
        ? this.comments
        : null;
    final phone = isPhoneLayout(context);
    // On an iPad pane that can draw on it: Draw, on a whole image Claktory
    // read (a cut one has no version for its ink to belong to).
    final draw = _commentable ? InkDraw.maybeOf(context)?.onDraw : null;
    // The ink saved on this version, over it, read-only (#362 I2).
    final inked = file != null && file.error.isEmpty && !file.truncated
        ? file.sha256
        : '';
    Widget glyphButton(String label, String icon, VoidCallback onTap) =>
        GlyphButton(
          label: label,
          active: false,
          onTap: onTap,
          glyph: (color) =>
              ExtensionGlyph(icon: icon, name: label, color: color),
        );
    final controls = <Widget>[
      if (draw != null)
        HeaderTextButton('Draw', draw, key: const ValueKey('image-draw')),
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
    if (problem != null || image == null || box == null) {
      body = problem ?? const SizedBox.shrink();
    } else {
      body = LayoutBuilder(
        key: _paneKey,
        builder: (context, constraints) {
          _pane = constraints.biggest;
          final fit = [
            1.0,
            (_pane.width - 2 * Space.l) / box.width,
            (_pane.height - 2 * Space.l) / box.height,
          ].reduce((a, b) => a < b ? a : b).clamp(0.01, 1.0);
          _fit = fit;
          // Scaled on every axis, as InteractiveViewer scales: its zoom
          // limits read the largest.
          if (_fitted) {
            final width = box.width * fit;
            final height = box.height * fit;
            _zoom.value = Matrix4.identity()
              ..translateByDouble(
                (_pane.width - width) / 2,
                (_pane.height - height) / 2,
                0,
                1,
              )
              ..scaleByDouble(fit, fit, fit, 1);
          }
          // The Comment button over the image takes its own clicks at
          // once, outside the image's double click.
          return Stack(
            children: [
              Positioned.fill(
                // Not while a comment or an answer is being typed.
                child: ViewerKeys(
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
                    const SingleActivator(
                      LogicalKeyboardKey.digit0,
                      meta: true,
                    ): _fitNow,
                    const SingleActivator(LogicalKeyboardKey.escape): () {
                      if (_marking) _toggleMarking();
                    },
                  },
                  child: Focus(
                    focusNode: _focus,
                    child: GestureDetector(
                      // While picking, a click is the spot at once: no
                      // waiting to see if it's a double click.
                      onTapUp: comments == null || _marking ? null : _tapped,
                      onDoubleTapDown: _marking
                          ? null
                          : (details) => _doubleTapAt = details.localPosition,
                      onDoubleTap: _marking ? null : _toggleActualSize,
                      child: Listener(
                        // A click takes the keys for zooming.
                        onPointerDown: (_) => _focus.requestFocus(),
                        // ⌘-scroll zooms about the pointer; a plain scroll pans,
                        // and a pinch zooms, as InteractiveViewer does.
                        onPointerSignal: (event) {
                          if (event is PointerScrollEvent &&
                              HardwareKeyboard.instance.isMetaPressed) {
                            _zoomTo(
                              _scale *
                                  (event.scrollDelta.dy < 0 ? 1.1 : 1 / 1.1),
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
                              boundaryMargin: const EdgeInsets.all(
                                double.infinity,
                              ),
                              onInteractionStart: (_) => _fitted = false,
                              child: SizedBox(
                                width: box.width,
                                height: box.height,
                                child: Stack(
                                  fit: StackFit.expand,
                                  children: [
                                    CustomPaint(
                                      painter: _Checkerboard(
                                        palette.hairline,
                                        zoom: _zoom,
                                      ),
                                      child: RawImage(
                                        image: image,
                                        width: box.width,
                                        height: box.height,
                                        fit: BoxFit.fill,
                                        filterQuality: _pixelScale > 2
                                            ? FilterQuality.none
                                            : FilterQuality.medium,
                                      ),
                                    ),
                                    // In the image's own box, so it zooms
                                    // with it; it takes no touches.
                                    if (inked.isNotEmpty)
                                      InkPreview(path: path, sha256: inked),
                                  ],
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
                                          : Rect.fromPoints(
                                              _dragFrom!,
                                              _dragTo!,
                                            ),
                                      signal: palette.signal,
                                      ring: palette.canvas,
                                      colorOf: (agentId) => agentColor(
                                        comments
                                                .backend
                                                .workspace
                                                .workspace
                                                .agents
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
                                  child: Listener(
                                    onPointerDown: (event) =>
                                        _fingers.add(event.pointer),
                                    onPointerUp: _fingerUp,
                                    onPointerCancel: _fingerUp,
                                    child: GestureDetector(
                                      behavior: HitTestBehavior.opaque,
                                      // From where the pointer went down, not
                                      // where the drag was recognised.
                                      dragStartBehavior: DragStartBehavior.down,
                                      onTapUp: (details) => _markSpot(
                                        _fraction(details.localPosition),
                                      ),
                                      onPanStart: phone ? null : _markStart,
                                      onPanUpdate: phone ? null : _markUpdate,
                                      onPanEnd: phone ? null : _markEnd,
                                      onScaleStart: phone ? _scaleStart : null,
                                      onScaleUpdate: phone
                                          ? _scaleUpdate
                                          : null,
                                      onScaleEnd: phone ? _scaleEnd : null,
                                    ),
                                  ),
                                ),
                              ),
                          ],
                        ),
                      ),
                    ),
                  ),
                ),
              ),
              // A phone has the bar under the image instead.
              if (comments != null && !phone)
                Positioned(
                  top: Space.m,
                  right: Space.m,
                  left: Space.m,
                  child: Align(
                    alignment: Alignment.topRight,
                    child: CommentPick(
                      picking: _marking,
                      visible: _hovered || _touch,
                      hint:
                          '${_touch ? 'Tap' : 'Click'} a spot or drag '
                          'across an area to comment on it.',
                      onToggle: _toggleMarking,
                    ),
                  ),
                ),
              // Whether the pointer is over the image, for the
              // Comment button; it takes no clicks.
              if (comments != null)
                Positioned.fill(
                  child: MouseRegion(
                    opaque: false,
                    hitTestBehavior: HitTestBehavior.translucent,
                    onEnter: (_) => setState(() => _hovered = true),
                    onExit: (_) => setState(() => _hovered = false),
                  ),
                ),
              // A preview says so, in the corner clear of the header's
              // buttons, Comment's and the bar under the image (#379).
              if (previewBadge() case final badge?)
                Positioned(
                  left: Space.m,
                  right: Space.m,
                  bottom: Space.m,
                  child: Align(alignment: Alignment.bottomLeft, child: badge),
                ),
            ],
          );
        },
      );
    }
    // Apple Pencil touching an image says how to draw on it, or, where
    // it can't be drawn on, how to comment instead (#362 I2). A touch
    // never starts drawing by itself: Draw is a deliberate tap.
    Widget pencil(Widget body) => image == null
        ? body
        : PencilHint(
            text: draw != null
                ? 'To draw on this image, tap Draw.'
                : comments == null
                ? 'You can draw only on PDFs.'
                : 'You can draw only on PDFs. To comment on this image, tap '
                      'Comment, then drag across it.',
            child: body,
          );
    return PaneScaffold(
      share: sharePath,
      details: details,
      controls: controls,
      comments: comments == null ? null : CommentsButton(controller: comments),
      actions: [
        if (draw != null) PaneAction('Draw', draw),
        if (image != null) ...[
          PaneAction('Fit', _fitNow),
          PaneAction('Actual size', () => _zoomTo(1)),
        ],
        PaneAction('Reload', reload),
      ],
      // A narrow iPad window: Draw beside Comment, until Comment is tapped.
      toolbar: image == null
          ? null
          : draw != null && !_marking
          ? DrawBar(
              onDraw: draw,
              onComment: comments == null ? null : _toggleMarking,
            )
          : comments == null
          ? null
          : CommentModeBar(
              marking: _marking,
              onToggle: _toggleMarking,
              hint: 'Tap a spot, or drag across an area.',
              action: 'Whole image',
              onAction: _markWhole,
            ),
      body: ClipRect(
        child: ColoredBox(
          color: palette.canvas,
          child: comments == null
              ? pencil(body)
              : Commented(controller: comments, child: pencil(body)),
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
    required this.ring,
    required this.colorOf,
  }) : super(repaint: Listenable.merge([comments, comments.selection, zoom]));

  final CommentsController comments;
  final TransformationController zoom;
  final Rect Function(Rect region) onScreen;
  final Rect? drag;
  final Color signal;

  /// Around each pin, so it shows on any picture: the canvas.
  final Color ring;
  final Color Function(String agentId) colorOf;

  @override
  void paint(Canvas canvas, Size size) {
    void box(Rect region, Color color, {required bool strong}) {
      final rect = onScreen(region);
      if (rect.isEmpty) {
        // A spot: only its pin.
        if (rect.width == 0 && rect.height == 0) {
          paintPin(canvas, rect.topLeft, color: color, ring: ring);
        }
        return;
      }
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

    final pins = comments.pins;
    final placed = <(Offset, Color, int, bool)>[];
    for (final thread in comments.shown) {
      final region = comments.regions[thread.id];
      if (region == null) continue;
      final color = colorOf(thread.agentId);
      final strong = comments.focused == thread.id;
      if (region.width > 0 || region.height > 0) {
        box(region, color, strong: strong);
      }
      final number = pins[thread.id];
      if (number != null) {
        placed.add((onScreen(region).topLeft, color, number, strong));
      }
    }
    // Pins over every outline, the picked one on top.
    placed.sort((a, b) => (a.$4 ? 1 : 0) - (b.$4 ? 1 : 0));
    for (final (at, color, number, strong) in placed) {
      paintPin(
        canvas,
        at,
        color: color,
        ring: ring,
        number: number,
        strong: strong,
      );
    }
    final selected = comments.selection.value;
    final writing = comments.composing;
    if (writing is Region) {
      box(writing.rect, signal, strong: false);
    } else if (selected is Region) {
      box(selected.rect, signal, strong: false);
    }
    final drag = this.drag;
    if (drag != null) box(drag, signal, strong: false);
  }

  @override
  bool shouldRepaint(_Regions old) =>
      old.drag != drag ||
      old.signal != signal ||
      old.ring != ring ||
      old.comments != comments;
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
