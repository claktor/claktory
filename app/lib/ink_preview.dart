// The ink drawn on an image, shown read-only wherever its pane is open
// (#362 I2): the backend keeps each ink layer with a preview, a
// transparent PNG of the ink alone in the image's own space, made for
// other devices to lay over the file. This lays the latest one of the
// version shown over the image, on every device, and follows saves from
// any device. Nothing is kept here: the backend is the truth.
//
// Drawing itself is the iPad's document host (ipad/document_host.dart):
// an image pane offers Draw only under [InkDraw].
import 'dart:async';
import 'dart:ui' as ui;

import 'package:flutter/widgets.dart';

import 'selection.dart' show Unselectable;
import 'backend.dart';
import 'controls.dart' show Button;
import 'identity.dart';
import 'ipad/ink_sync.dart' show InkBlobs;
import 'protocol/factory/v1/factory.pb.dart';
import 'mobile/metrics.dart' show Touch;
import 'scope.dart';

/// Above an image's viewer in an iPad pane that can draw on it: [onDraw]
/// turns the pane into the drawing surface.
class InkDraw extends InheritedWidget {
  const InkDraw({super.key, required this.onDraw, required super.child});

  final VoidCallback onDraw;

  static InkDraw? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<InkDraw>();

  @override
  bool updateShouldNotify(InkDraw old) => old.onDraw != onDraw;
}

/// Under an image on an iPad window narrowed to the phone's routes, where
/// the header has no room: Draw, and Comment beside it when the image can
/// be commented on (as the phone's Comment bar, at rest); while it's drawn
/// on, Done.
class DrawBar extends StatelessWidget {
  const DrawBar({super.key, required this.onDraw, this.onComment})
    : onDone = null;

  /// While drawing: Done alone, back to the image.
  const DrawBar.done({super.key, required VoidCallback this.onDone})
    : onDraw = null,
      onComment = null;

  final VoidCallback? onDraw;
  final VoidCallback? onComment;
  final VoidCallback? onDone;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final onComment = this.onComment;
    return Unselectable(
      child: Container(
        key: const ValueKey('draw-bar'),
        padding: const EdgeInsets.symmetric(
          horizontal: Touch.gutter,
          vertical: Space.xs,
        ),
        decoration: BoxDecoration(
          color: palette.sidebar,
          border: Border(top: BorderSide(color: palette.hairline)),
        ),
        // At the largest text, one over the other, Draw on top.
        child: OverflowBar(
          alignment: MainAxisAlignment.end,
          spacing: Space.s,
          overflowSpacing: Space.xs,
          overflowAlignment: OverflowBarAlignment.end,
          overflowDirection: VerticalDirection.up,
          children: [
            if (onComment != null)
              Button(label: 'Comment', onPressed: onComment),
            if (onDraw case final onDraw?)
              Button(label: 'Draw', primary: true, onPressed: onDraw),
            if (onDone case final onDone?)
              Button(label: 'Done', primary: true, onPressed: onDone),
          ],
        ),
      ),
    );
  }
}

/// The layer of [layers] to show over the version [sha256] of the file: the
/// image's own (not a copy kept beside another after a conflict), at its
/// latest revision; null when there's none.
@visibleForTesting
InkLayer? shownInkLayer(Iterable<InkLayer> layers, String sha256) {
  InkLayer? best;
  for (final layer in layers) {
    if (layer.sourceSha256 != sha256 ||
        layer.conflictOf.isNotEmpty ||
        layer.space.kind != 'image' ||
        layer.preview.sha256.isEmpty) {
      continue;
    }
    if (best == null ||
        layer.revision > best.revision ||
        (layer.revision == best.revision && layer.updatedMs > best.updatedMs)) {
      best = layer;
    }
  }
  return best;
}

/// The saved ink on the image at [path], version [sha256], stretched over
/// the box it's given (the image's own size): nothing until there's some,
/// and nothing on a Claktory that keeps no drawings. Takes no touches.
class InkPreview extends StatefulWidget {
  const InkPreview({super.key, required this.path, required this.sha256});

  final String path;
  final String sha256;

  @override
  State<InkPreview> createState() => _InkPreviewState();
}

class _InkPreviewState extends State<InkPreview> {
  Backend? _backend;

  /// [InkPreview.path] as the backend names it, from its first answer.
  String? _canonical;

  /// The preview shown and its blob's name.
  ui.Image? _image;
  String? _shownSha;

  /// The preview being read, so a newer one wins.
  String? _wanted;

  /// The last list failed as a request (not connected): asked again when
  /// the Claktory connects.
  var _retry = false;
  var _unsupported = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final backend = AppScope.of(context);
    if (backend == _backend) return;
    _backend?.inkLayerLists.removeListener(_listed);
    _backend?.connected.removeListener(_connectedChanged);
    _backend = backend
      ..inkLayerLists.addListener(_listed)
      ..connected.addListener(_connectedChanged);
    _canonical = null;
    unawaited(_list());
  }

  @override
  void didUpdateWidget(InkPreview old) {
    super.didUpdateWidget(old);
    if (old.path != widget.path) {
      _canonical = null;
      unawaited(_list());
    } else if (old.sha256 != widget.sha256) {
      // Another version shows: its own ink, if any.
      final list = _backend?.inkLayerLists.value;
      if (list != null && list.path == _canonical) {
        _show(list);
      } else {
        unawaited(_list());
      }
    }
  }

  Future<void> _list() async {
    final backend = _backend;
    if (backend == null || _unsupported) return;
    final path = widget.path;
    try {
      final list = await backend.listInkLayers(path);
      if (!mounted || path != widget.path) return;
      _retry = false;
      _canonical = list.path;
      _show(list);
    } on RequestFailure catch (failure) {
      if (failure.code == ErrorCode.ERROR_CODE_UNKNOWN_MESSAGE) {
        // A Claktory before ink layers: nothing to show, ever.
        _unsupported = true;
      } else {
        _retry = true;
      }
    } on Object {
      _retry = true;
    }
  }

  void _connectedChanged() {
    if ((_backend?.connected.value ?? false) && _retry) unawaited(_list());
  }

  /// A layer of this file was saved or deleted, on any device.
  void _listed() {
    final list = _backend?.inkLayerLists.value;
    if (list == null || !mounted) return;
    if (list.path != (_canonical ?? widget.path)) return;
    _show(list);
  }

  void _show(InkLayerList list) {
    final layer = shownInkLayer(list.layers, widget.sha256);
    final sha = layer?.preview.sha256;
    if (sha == _shownSha && _wanted == null) return;
    if (sha == null) {
      _wanted = null;
      setState(_drop);
      return;
    }
    if (sha == _wanted) return;
    _wanted = sha;
    unawaited(_read(sha));
  }

  Future<void> _read(String sha) async {
    final backend = _backend;
    if (backend == null) return;
    ui.Image? image;
    try {
      final bytes = await InkBlobs(backend).read(sha);
      final codec = await ui.instantiateImageCodec(bytes);
      image = (await codec.getNextFrame()).image;
    } on Object {
      // Unreadable now: what was shown stays, and the next save tries
      // again.
      if (_wanted == sha) _wanted = null;
      return;
    }
    if (!mounted || _wanted != sha) {
      image.dispose();
      return;
    }
    setState(() {
      _drop();
      _image = image;
      _shownSha = sha;
      _wanted = null;
    });
  }

  void _drop() {
    _image?.dispose();
    _image = null;
    _shownSha = null;
  }

  @override
  void dispose() {
    _backend?.inkLayerLists.removeListener(_listed);
    _backend?.connected.removeListener(_connectedChanged);
    _image?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final image = _image;
    if (image == null) return const SizedBox.shrink();
    return IgnorePointer(
      child: RawImage(
        key: const ValueKey('ink-preview'),
        image: image,
        fit: BoxFit.fill,
        filterQuality: FilterQuality.medium,
      ),
    );
  }
}
