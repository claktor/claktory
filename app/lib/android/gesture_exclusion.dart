// Where Android's Back swipe doesn't start.
//
// With gesture navigation, a swipe in from either side edge is Back. The
// phone's hold-to-record mic sits near the right edge, so holding it and
// sliding toward the start to cancel began a Back instead: the app left
// and the recording was lost. Android decides a swipe is Back as the
// finger comes down, so the mic's place has to be taken out of the gesture
// for as long as the mic is on screen, not only while it's held. The host
// (factory/back-gesture, BackGestureExclusion.kt) sets the window's
// exclusion rects; this keeps them where the wrapped widgets are.
//
// Only each wrapped widget's own box is excluded: Back still works
// everywhere else on the edge. Off Android it does nothing.

import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

/// Takes [child]'s box out of Android's Back swipe while it's on screen.
/// Touches pass through to [child] as they would without it.
class AndroidBackGestureExclusion extends SingleChildRenderObjectWidget {
  const AndroidBackGestureExclusion({super.key, super.child});

  @override
  RenderObject createRenderObject(BuildContext context) =>
      RenderBackGestureExclusion(_onAndroid);

  @override
  void updateRenderObject(
    BuildContext context,
    RenderBackGestureExclusion renderObject,
  ) {
    renderObject.enabled = _onAndroid;
  }

  static bool get _onAndroid =>
      !kIsWeb && defaultTargetPlatform == TargetPlatform.android;
}

/// [AndroidBackGestureExclusion]'s box: its child's, reported where it is.
class RenderBackGestureExclusion extends RenderProxyBox {
  RenderBackGestureExclusion(this._enabled);

  bool _enabled;
  set enabled(bool value) {
    if (value == _enabled) return;
    _enabled = value;
    if (value) {
      BackGestureExclusions.instance._add(this);
    } else {
      BackGestureExclusions.instance._remove(this);
    }
  }

  /// Where it is now, in the window's logical pixels; null while it has
  /// no place on screen.
  Rect? get place {
    if (!attached || !hasSize || size.isEmpty) return null;
    return MatrixUtils.transformRect(getTransformTo(null), Offset.zero & size);
  }

  @override
  void attach(PipelineOwner owner) {
    super.attach(owner);
    if (_enabled) BackGestureExclusions.instance._add(this);
  }

  @override
  void detach() {
    BackGestureExclusions.instance._remove(this);
    super.detach();
  }
}

/// Every wrapped widget's box, sent to the host as one list whenever it
/// changes: after each frame (a keyboard, a rotation or a scroll moves
/// them, and moving takes a frame), and when one comes or goes, so
/// removing one never gives back another's place.
class BackGestureExclusions {
  BackGestureExclusions._();

  static final instance = BackGestureExclusions._();

  static const _channel = MethodChannel('factory/back-gesture');

  final _boxes = <RenderBackGestureExclusion>{};
  List<Rect> _sent = const [];
  bool _checking = false;

  /// What the host was last told, for tests.
  @visibleForTesting
  List<Rect> get sent => _sent;

  /// Forgets what was sent, between tests.
  @visibleForTesting
  void reset() {
    _boxes.clear();
    _sent = const [];
    _checking = false;
  }

  void _add(RenderBackGestureExclusion box) {
    _boxes.add(box);
    _checkAfterFrame(soon: true);
  }

  void _remove(RenderBackGestureExclusion box) {
    if (!_boxes.remove(box)) return;
    _checkAfterFrame(soon: true);
  }

  /// Checks after the next frame. [soon] asks for one (a box came or
  /// went); otherwise it waits for whatever frame comes, so a still
  /// screen draws nothing for it.
  void _checkAfterFrame({bool soon = false}) {
    if (soon) SchedulerBinding.instance.ensureVisualUpdate();
    if (_checking) return;
    _checking = true;
    SchedulerBinding.instance.addPostFrameCallback((_) {
      _checking = false;
      _check();
      // Keep looking while there's something to keep in place.
      if (_boxes.isNotEmpty) _checkAfterFrame();
    });
  }

  void _check() {
    final now = [for (final box in _boxes) ?box.place];
    if (listEquals(now, _sent)) return;
    _sent = now;
    _channel
        .invokeMethod<bool>('exclude', {
          'rects': [
            for (final rect in now)
              [rect.left, rect.top, rect.width, rect.height],
          ],
        })
        .catchError((Object _) => false);
  }
}
