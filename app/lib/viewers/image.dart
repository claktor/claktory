import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import '../pane_chrome.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../resources.dart' show formatBytes;
import '../selection.dart';
import 'registry.dart';

/// An image: fitted to the pane (never enlarged past its pixels), over a
/// checkerboard where it's transparent, with its pixel size. Zoom with a
/// pinch, ⌘-scroll, ⌘+ ⌘− ⌘0 once clicked, or the header's controls; a
/// double click goes to 100% about the pointer and back to fitted; drag or
/// scroll to move.
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

  void _zoomChanged() {
    final phase = SchedulerBinding.instance.schedulerPhase;
    if (phase == SchedulerPhase.idle ||
        phase == SchedulerPhase.postFrameCallbacks) {
      _shownScale.value = _scale;
    } else {
      SchedulerBinding.instance.addPostFrameCallback((_) {
        if (mounted) _shownScale.value = _scale;
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
    } on Exception {
      if (mounted) {
        setState(() => _undecodable = "Claktory can't open this image.");
      }
    }
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
    final details = [
      if (image != null) '${image.width} × ${image.height}',
      if (file != null && file.error.isEmpty) formatBytes(file.size),
    ].join(' · ');
    Widget glyphButton(String label, String icon, VoidCallback onTap) =>
        GlyphButton(
          label: label,
          active: false,
          onTap: onTap,
          glyph: (color) =>
              ExtensionGlyph(icon: icon, name: label, color: color),
        );
    final controls = <Widget>[
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
        _TextButton('Fit', _fitNow),
        _TextButton('100%', () => _zoomTo(1)),
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
            },
            child: Focus(
              focusNode: _focus,
              child: GestureDetector(
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
                  child: InteractiveViewer(
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
                        painter: _Checkerboard(palette.hairline, zoom: _zoom),
                        child: RawImage(
                          image: image,
                          filterQuality: _scale > 2
                              ? FilterQuality.none
                              : FilterQuality.medium,
                        ),
                      ),
                    ),
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
        child: ColoredBox(color: palette.canvas, child: body),
      ),
    );
  }
}

/// A quiet text button for a pane's header: "Fit", "100%".
class _TextButton extends StatelessWidget {
  const _TextButton(this.label, this.onTap);

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Unselectable(
      child: Semantics(
        button: true,
        label: label,
        excludeSemantics: true,
        child: Pressable(
          onTap: onTap,
          builder: (context, hovered) => Container(
            height: 28,
            padding: const EdgeInsets.symmetric(horizontal: Space.s),
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: hovered ? palette.hover : const Color(0x00000000),
              borderRadius: BorderRadius.circular(Radii.control),
            ),
            child: Text(
              label,
              style: TextStyles.caption.copyWith(
                color: hovered ? palette.text : palette.textMuted,
                fontWeight: FontWeight.w500,
              ),
            ),
          ),
        ),
      ),
    );
  }
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
