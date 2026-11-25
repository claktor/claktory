import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/gestures.dart';
import 'package:flutter/physics.dart';
import 'package:flutter/widgets.dart';

import '../identity.dart' show isPhoneLayout;
import 'haptics.dart';

/// On a phone, a picture or video pulled down closes, as in Photos: the
/// child follows one finger, shrinking toward 0.85 while [backdrop] fades,
/// and let go past a fifth of the height (or flung down) it calls
/// [onClose] with one haptic; short of that it springs back, critically
/// damped (docs/IDENTITY.md, Motion).
///
/// It claims only a vertical drag of one finger: a tap, a horizontal drag
/// (pages, the edge's Back) and two fingers (a pinch) go to the child. A
/// second finger landing mid-drag gives the drag up and springs back.
/// Nothing at all while [enabled] is false (zoomed in, commenting), under
/// VoiceOver or TalkBack (Back is there), or outside a phone's layout. With
/// reduced motion the child doesn't move or shrink: it fades.
class SwipeToClose extends StatefulWidget {
  const SwipeToClose({
    super.key,
    required this.child,
    required this.onClose,
    this.enabled = true,
    this.progress,
    this.backdrop,
  });

  final Widget child;

  /// The page's Back.
  final VoidCallback onClose;

  /// False while zoomed in, commenting or answering: everything goes to the
  /// child.
  final bool enabled;

  /// 0 at rest, toward 1 as it's pulled down (0.5 where letting go
  /// closes): for the page's chrome to fade with it.
  final ValueNotifier<double>? progress;

  /// Drawn behind the child, fading out as it's pulled.
  final Color? backdrop;

  /// How far down, as a share of the height, a pull closes when let go.
  static const closeAt = 0.2;

  /// How fast a downward fling closes, in points a second.
  static const flingAt = 700.0;

  /// The smallest the child gets.
  static const smallest = 0.85;

  @override
  State<SwipeToClose> createState() => _SwipeToCloseState();
}

class _SwipeToCloseState extends State<SwipeToClose>
    with SingleTickerProviderStateMixin {
  /// Where the child is, from its place: follows the finger, then springs
  /// back from where it was let go.
  Offset _offset = Offset.zero;

  /// Where the finger went down, and where the child was then.
  Offset _from = Offset.zero;
  Offset _start = Offset.zero;

  var _dragging = false;

  /// Given up for this drag (a second finger): its updates are ignored.
  var _dropped = false;
  var _closed = false;
  final _pointers = <int>{};
  double _height = 1;

  late final AnimationController _back =
      AnimationController.unbounded(vsync: this)
        ..addListener(_springing)
        ..addStatusListener((status) {
          // Settled: exactly in its place.
          if (status == AnimationStatus.completed && mounted) {
            setState(() => _offset = Offset.zero);
            _show();
          }
        });
  Offset _springFrom = Offset.zero;

  @override
  void dispose() {
    _back.dispose();
    super.dispose();
  }

  bool get _reduced => MediaQuery.disableAnimationsOf(context);

  /// How far it's pulled, 0 to 1: down only, a close at half of it.
  double get _pulled =>
      (_offset.dy / (_height * SwipeToClose.closeAt * 2)).clamp(0.0, 1.0);

  void _show() {
    widget.progress?.value = _pulled;
  }

  void _springing() {
    setState(() => _offset = _springFrom * _back.value);
    _show();
  }

  void _down(DragDownDetails details) {
    _back.stop();
    _dropped = false;
  }

  void _began(DragStartDetails details) {
    if (_closed) return;
    _back.stop();
    _dragging = true;
    _dropped = false;
    _from = details.globalPosition;
    _start = _offset;
  }

  void _moved(DragUpdateDetails details) {
    if (!_dragging || _dropped || _closed) return;
    var moved = details.globalPosition - _from;
    // Up follows the finger, but less and less: it never closes.
    if (_start.dy + moved.dy < 0) {
      final up = -(_start.dy + moved.dy);
      moved = Offset(moved.dx, -_start.dy - math.sqrt(up) * 4);
    }
    setState(() => _offset = _start + moved);
    _show();
  }

  void _ended(DragEndDetails details) {
    if (!_dragging) return;
    _dragging = false;
    if (_dropped || _closed) return;
    final down = details.velocity.pixelsPerSecond.dy;
    final far = _offset.dy > _height * SwipeToClose.closeAt;
    // A fling up after a pull doesn't close.
    if ((far && down > -SwipeToClose.flingAt) || down > SwipeToClose.flingAt) {
      _closed = true;
      unawaited(Haptics.light());
      widget.onClose();
      return;
    }
    _springBack(down);
  }

  void _cancelled() {
    if (!_dragging) return;
    _dragging = false;
    _springBack(0);
  }

  void _springBack(double velocity) {
    if (_offset == Offset.zero) return;
    if (_reduced) {
      setState(() => _offset = Offset.zero);
      _show();
      return;
    }
    _springFrom = _offset;
    final distance = _offset.distance;
    // A critically damped spring from 1 (where it was let go) to 0, with
    // the finger's speed along the way back.
    final speed = distance == 0 ? 0.0 : -velocity / distance;
    _back
      ..value = 1
      ..animateWith(
        SpringSimulation(
          SpringDescription.withDampingRatio(mass: 1, stiffness: 400),
          1,
          0,
          speed,
        ),
      );
  }

  void _pointerDown(PointerDownEvent event) {
    _pointers.add(event.pointer);
    // A second finger: a pinch, not a pull.
    if (_pointers.length > 1 && _dragging && !_dropped) {
      _dropped = true;
      _dragging = false;
      _springBack(0);
    }
  }

  void _pointerUp(PointerEvent event) => _pointers.remove(event.pointer);

  @override
  Widget build(BuildContext context) {
    final on =
        widget.enabled &&
        !MediaQuery.accessibleNavigationOf(context) &&
        isPhoneLayout(context);
    if (!on && (_offset != Offset.zero || _dragging)) {
      // Turned off mid-pull: back at once.
      _back.stop();
      _dragging = false;
      _offset = Offset.zero;
      WidgetsBinding.instance.addPostFrameCallback((_) => _show());
    }
    final pulled = _pulled;
    final reduced = _reduced;
    Widget child = widget.child;
    if (reduced) {
      child = Opacity(opacity: 1 - pulled * 0.6, child: child);
    } else if (_offset != Offset.zero) {
      child = Transform.translate(
        offset: _offset,
        child: Transform.scale(
          scale: 1 - (1 - SwipeToClose.smallest) * pulled,
          child: child,
        ),
      );
    }
    return LayoutBuilder(
      builder: (context, constraints) {
        _height = constraints.maxHeight.isFinite && constraints.maxHeight > 0
            ? constraints.maxHeight
            : MediaQuery.sizeOf(context).height;
        return Listener(
          onPointerDown: _pointerDown,
          onPointerUp: _pointerUp,
          onPointerCancel: _pointerUp,
          behavior: HitTestBehavior.translucent,
          child: RawGestureDetector(
            behavior: HitTestBehavior.translucent,
            gestures: {
              if (on)
                _OneFingerVerticalDrag:
                    GestureRecognizerFactoryWithHandlers<
                      _OneFingerVerticalDrag
                    >(_OneFingerVerticalDrag.new, (drag) {
                      drag
                        ..onDown = _down
                        ..onStart = _began
                        ..onUpdate = _moved
                        ..onEnd = _ended
                        ..onCancel = _cancelled;
                    }),
            },
            child: Stack(
              fit: StackFit.passthrough,
              children: [
                if (widget.backdrop case final backdrop?)
                  Positioned.fill(
                    child: IgnorePointer(
                      child: ColoredBox(
                        color: backdrop.withValues(
                          alpha: backdrop.a * (1 - pulled),
                        ),
                      ),
                    ),
                  ),
                child,
              ],
            ),
          ),
        );
      },
    );
  }
}

/// A vertical drag of one finger only: a second finger before it has won
/// gives it up, so a pinch goes to the child.
class _OneFingerVerticalDrag extends VerticalDragGestureRecognizer {
  _OneFingerVerticalDrag()
    : super(
        supportedDevices: {PointerDeviceKind.touch, PointerDeviceKind.stylus},
      );

  final _down = <int>{};
  var _won = false;

  @override
  void addAllowedPointer(PointerDownEvent event) {
    _down.add(event.pointer);
    if (_down.length > 1) {
      // Before it has won: given up, both fingers the child's. After: the
      // drag is dropped (SwipeToClose's own count), and this finger isn't
      // claimed.
      if (!_won) resolve(GestureDisposition.rejected);
      return;
    }
    super.addAllowedPointer(event);
  }

  @override
  void acceptGesture(int pointer) {
    _won = true;
    super.acceptGesture(pointer);
  }

  @override
  void didStopTrackingLastPointer(int pointer) {
    _down.clear();
    _won = false;
    super.didStopTrackingLastPointer(pointer);
  }

  @override
  void rejectGesture(int pointer) {
    _down.remove(pointer);
    super.rejectGesture(pointer);
  }
}
