import 'dart:async';
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../identity.dart';
import 'live_input.dart';

// An agent's browser, live: the frames of its screen as they come, and,
// in a tab handed to the person to solve a check, their pointer, wheel and
// keys back to it. Frames are drawn only while a view is open, and each is
// acknowledged once it's drawn, so a slow device gets fewer frames, never
// a backlog. Nothing is kept: the frame shown is the only one in memory.

/// One frame of the browser's screen, decoded.
@immutable
class LiveFrame {
  const LiveFrame({
    required this.seq,
    required this.image,
    this.origin = '',
    this.title = '',
    this.textFieldFocused = false,
  });

  final int seq;
  final ui.Image image;

  /// The site and the page shown, read by Claktory, not by the page.
  final String origin;
  final String title;

  /// A text field has the focus (known in a driven session): a phone's
  /// keyboard comes up.
  final bool textFieldFocused;

  Size get size => Size(image.width.toDouble(), image.height.toDouble());
}

/// The frame to show: the newest decoded, each acknowledged ([ack]) once
/// it's ready to draw. A frame that comes while an older one is still
/// decoding replaces it; an older one that finishes after a newer one is
/// dropped.
class LiveFrames extends ChangeNotifier {
  LiveFrames({required this.ack, this.decode = _decode});

  /// Tells the backend the frame [seq] is drawn, so the next may come.
  final void Function(int seq) ack;

  /// Decodes a JPEG (a test's stand-in).
  final Future<ui.Image> Function(Uint8List bytes) decode;

  LiveFrame? get frame => _frame;
  LiveFrame? _frame;
  var _newest = -1;
  var _disposed = false;

  static Future<ui.Image> _decode(Uint8List bytes) async {
    final codec = await ui.instantiateImageCodec(bytes);
    try {
      return (await codec.getNextFrame()).image;
    } finally {
      codec.dispose();
    }
  }

  /// Takes frame [seq]'s [jpeg].
  Future<void> take(
    int seq,
    Uint8List jpeg, {
    String origin = '',
    String title = '',
    bool textFieldFocused = false,
  }) async {
    if (seq <= _newest) return;
    _newest = seq;
    final ui.Image image;
    try {
      image = await decode(jpeg);
    } on Object {
      // A frame that doesn't decode is skipped; the next one may.
      if (!_disposed && seq == _newest) ack(seq);
      return;
    }
    if (_disposed || seq != _newest) {
      image.dispose();
      return;
    }
    _frame?.image.dispose();
    _frame = LiveFrame(
      seq: seq,
      image: image,
      origin: origin,
      title: title,
      textFieldFocused: textFieldFocused,
    );
    notifyListeners();
    ack(seq);
  }

  /// The view closed: nothing more to show.
  void clear() {
    _frame?.image.dispose();
    _frame = null;
    _newest = -1;
    notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _frame?.image.dispose();
    _frame = null;
    super.dispose();
  }
}

/// The browser's screen: [frames] drawn as large as they fit, and while
/// [acting], what the person does with the pointer, the wheel and the keys
/// sent as [onInput]: what arrives together goes together, with only the
/// latest pointer move of it. Watching only, nothing is sent.
///
/// With [zoom] (a touch screen), two fingers pinch and pan the frame from
/// 1× to 4× and a double tap goes between the fitted frame and 2.5× at the
/// tap; a tap clicks where it lands, and a long press then a drag drags.
class LiveScreen extends StatefulWidget {
  const LiveScreen({
    super.key,
    required this.frames,
    required this.acting,
    required this.onInput,
    this.mac = false,
    this.maxScale = double.infinity,
    this.alignment = Alignment.center,
    this.zoom = false,
    this.focusNode,
  });

  final LiveFrames frames;
  final bool acting;
  final void Function(List<LiveEvent> events) onInput;

  /// Keys as on a Mac: ⌘ for shortcuts.
  final bool mac;

  /// The most the frame is drawn at, times its own size: one frame pixel
  /// to a screen pixel is 1 / the device's pixel ratio.
  final double maxScale;

  /// Where the frame sits in the room left around it.
  final Alignment alignment;

  /// Pinch, pan and double tap zoom the frame; touches click and drag.
  final bool zoom;
  final FocusNode? focusNode;

  @override
  State<LiveScreen> createState() => _LiveScreenState();
}

/// The most a pinch zooms the frame.
const liveZoomMost = 4.0;

/// What a double tap zooms the fitted frame to.
const liveZoomTap = 2.5;

class _LiveScreenState extends State<LiveScreen> {
  final _pending = <LiveEvent>[];
  LivePointer? _move;
  var _flushing = false;
  var _buttons = 0;

  /// A press went down on the frame: its moves and its release go too,
  /// wherever they are.
  var _pressed = false;
  FocusNode? _ownFocus;
  final _zoom = TransformationController();
  Offset? _doubleTapAt;

  FocusNode get _focus => widget.focusNode ?? (_ownFocus ??= FocusNode());

  @override
  void dispose() {
    _ownFocus?.dispose();
    _zoom.dispose();
    super.dispose();
  }

  void _send(LiveEvent event) {
    if (!widget.acting) return;
    if (event is LivePointer && event.kind == LivePointerKind.move) {
      // Only the latest move in a frame matters.
      _move = event;
    } else {
      if (_move case final move?) _pending.add(move);
      _move = null;
      _pending.add(event);
    }
    if (_flushing) return;
    _flushing = true;
    scheduleMicrotask(_flush);
  }

  void _flush() {
    _flushing = false;
    if (_move case final move?) _pending.add(move);
    _move = null;
    if (_pending.isEmpty || !mounted) return;
    final events = List<LiveEvent>.of(_pending);
    _pending.clear();
    widget.onInput(events);
  }

  Rect _rect(Size box, Size frame) => liveFrameRect(
    box,
    frame,
    maxScale: widget.maxScale,
    alignment: widget.alignment,
  );

  Offset? _at(Offset local, Size box) {
    final frame = widget.frames.frame;
    return frame == null
        ? null
        : liveFramePoint(
            local,
            box,
            frame.size,
            maxScale: widget.maxScale,
            alignment: widget.alignment,
          );
  }

  static LiveButton _button(int buttons) => buttons & kSecondaryButton != 0
      ? LiveButton.right
      : buttons & kMiddleMouseButton != 0
      ? LiveButton.middle
      : LiveButton.left;

  KeyEventResult _key(FocusNode node, KeyEvent event) {
    // Esc is Claktory's: it leaves the view.
    if (!widget.acting || event.logicalKey == LogicalKeyboardKey.escape) {
      return KeyEventResult.ignored;
    }
    final events = liveKeyEvents(event, mac: widget.mac);
    for (final live in events) {
      _send(live);
    }
    return KeyEventResult.handled;
  }

  /// The frame as it's drawn now: nothing before the first.
  Widget _picture(BuildContext context, Size box) => ListenableBuilder(
    listenable: widget.frames,
    builder: (context, _) {
      final frame = widget.frames.frame;
      if (frame == null) {
        return ColoredBox(color: Palette.of(context).sidebar);
      }
      return MouseRegion(
        cursor: widget.acting
            ? SystemMouseCursors.basic
            : SystemMouseCursors.forbidden,
        child: Stack(
          children: [
            Positioned.fromRect(
              rect: _rect(box, frame.size),
              child: RawImage(
                image: frame.image,
                fit: BoxFit.fill,
                filterQuality: FilterQuality.medium,
              ),
            ),
          ],
        ),
      );
    },
  );

  /// A double tap: back to the fitted frame, or [liveZoomTap] at the tap.
  void _toggleZoom() {
    final at = _doubleTapAt;
    if (_zoom.value.getMaxScaleOnAxis() > 1.01 || at == null) {
      _zoom.value = Matrix4.identity();
      return;
    }
    _zoom.value = Matrix4.identity()
      ..translateByDouble(
        -at.dx * (liveZoomTap - 1),
        -at.dy * (liveZoomTap - 1),
        0,
        1,
      )
      ..scaleByDouble(liveZoomTap, liveZoomTap, 1, 1);
  }

  void _touch(LivePointerKind kind, Offset local, Size box) {
    final at = kind == LivePointerKind.down
        ? _at(local, box)
        : _clampedAt(local, box);
    if (at == null) return;
    _send(LivePointer(kind, at.dx, at.dy));
  }

  /// A touch screen's: zoomed with two fingers, clicked with one.
  Widget _zoomed(BuildContext context, Size box) => InteractiveViewer(
    transformationController: _zoom,
    maxScale: liveZoomMost,
    child: GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTapDown: (_) => _focus.requestFocus(),
      onTapUp: (details) {
        if (_at(details.localPosition, box) == null) return;
        _touch(LivePointerKind.down, details.localPosition, box);
        _touch(LivePointerKind.up, details.localPosition, box);
      },
      onDoubleTapDown: (details) => _doubleTapAt = details.localPosition,
      onDoubleTap: _toggleZoom,
      onLongPressStart: (details) {
        _pressed = _at(details.localPosition, box) != null;
        if (_pressed) _touch(LivePointerKind.down, details.localPosition, box);
      },
      onLongPressMoveUpdate: (details) {
        if (_pressed) _touch(LivePointerKind.move, details.localPosition, box);
      },
      onLongPressEnd: (details) {
        if (!_pressed) return;
        _pressed = false;
        _touch(LivePointerKind.up, details.localPosition, box);
      },
      child: SizedBox.fromSize(size: box, child: _picture(context, box)),
    ),
  );

  @override
  Widget build(BuildContext context) => Focus(
    focusNode: _focus,
    onKeyEvent: _key,
    child: LayoutBuilder(
      builder: (context, constraints) {
        final box = constraints.biggest;
        if (widget.zoom) return _zoomed(context, box);
        return Listener(
          behavior: HitTestBehavior.opaque,
          onPointerDown: (event) {
            _focus.requestFocus();
            final at = _at(event.localPosition, box);
            if (at == null) return;
            _buttons = event.buttons;
            _pressed = true;
            _send(
              LivePointer(
                LivePointerKind.down,
                at.dx,
                at.dy,
                button: _button(event.buttons),
              ),
            );
          },
          onPointerMove: (event) {
            if (!_pressed) return;
            final at = _clampedAt(event.localPosition, box);
            if (at == null) return;
            _send(
              LivePointer(
                LivePointerKind.move,
                at.dx,
                at.dy,
                button: _button(_buttons),
              ),
            );
          },
          onPointerHover: (event) {
            final at = _at(event.localPosition, box);
            if (at == null) return;
            _send(LivePointer(LivePointerKind.move, at.dx, at.dy));
          },
          onPointerUp: (event) {
            if (!_pressed) return;
            _pressed = false;
            final at = _clampedAt(event.localPosition, box);
            final button = _button(_buttons);
            _buttons = 0;
            if (at == null) return;
            _send(
              LivePointer(LivePointerKind.up, at.dx, at.dy, button: button),
            );
          },
          onPointerCancel: (_) => _pressed = false,
          onPointerSignal: (signal) {
            if (signal is! PointerScrollEvent) return;
            final at = _at(signal.localPosition, box);
            final frame = widget.frames.frame;
            if (at == null || frame == null) return;
            // The wheel in the screen's pixels: the frame's are the
            // screen's, or a scaled copy the runner scales back.
            final scale = frame.size.width / _rect(box, frame.size).width;
            _send(
              LiveWheel(
                signal.scrollDelta.dx * scale,
                signal.scrollDelta.dy * scale,
                at.dx,
                at.dy,
              ),
            );
          },
          child: _picture(context, box),
        );
      },
    ),
  );

  /// [local] moved onto the frame's nearest edge: a drag let go outside
  /// still ends where it left.
  Offset? _clampedAt(Offset local, Size box) {
    final frame = widget.frames.frame;
    if (frame == null) return null;
    final rect = _rect(box, frame.size);
    if (rect.isEmpty) return null;
    return Offset(
      ((local.dx - rect.left) / rect.width).clamp(0.0, 1.0),
      ((local.dy - rect.top) / rect.height).clamp(0.0, 1.0),
    );
  }
}
