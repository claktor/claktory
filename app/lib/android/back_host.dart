import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'back_policy.dart';

// SystemNavigator's Back-ownership flag is engine-global. A replacement host
// owns publication immediately, before the old host is disposed at frame end.
int _latestHostGeneration = 0;

/// The single Android Back dispatcher around the shared phone registry.
///
/// It belongs above the drawer and nested Navigator. Shared overlays may be
/// siblings in the window Overlay: they still participate via [changes].
/// Native storage, a second Navigator and product state do not belong here.
class AndroidBackHost<T extends Object> extends StatefulWidget {
  const AndroidBackHost({
    super.key,
    required this.changes,
    required this.readTarget,
    required this.canClose,
    required this.commitTarget,
    required this.pageKey,
    required this.child,
  });

  final Listenable changes;
  final T? Function() readTarget;
  final bool Function(T) canClose;
  final void Function(T) commitTarget;
  final LocalKey? Function(T) pageKey;
  final Widget child;

  @override
  State<AndroidBackHost<T>> createState() => _AndroidBackHostState<T>();
}

class _AndroidBackHostState<T extends Object> extends State<AndroidBackHost<T>>
    with WidgetsBindingObserver {
  late final _back = AndroidBackCoordinator<T>(
    readTarget: () => widget.readTarget(),
    canClose: (target) => widget.canClose(target),
    commitTarget: (target) => widget.commitTarget(target),
  );
  final _motion = ValueNotifier<_PageMotion?>(null);
  AndroidBackAttempt<T>? _attempt;
  bool _notificationPending = false;
  late final int _generation;

  bool get _current => _generation == _latestHostGeneration;

  // Raw view metrics, not a page's already-consumed MediaQuery or keyboard
  // focus (which remains true with a hardware keyboard or after IME dismissal).
  bool get _imeVisible => View.of(context).viewInsets.bottom > 0;

  @override
  void initState() {
    super.initState();
    _generation = ++_latestHostGeneration;
    widget.changes.addListener(_changed);
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _publishEligibility();
  }

  @override
  void didUpdateWidget(AndroidBackHost<T> oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.changes != widget.changes) {
      _finish(committed: false);
      oldWidget.changes.removeListener(_changed);
      widget.changes.addListener(_changed);
    }
    _changed();
  }

  void _changed() {
    final attempt = _attempt;
    if (attempt != null && attempt.target != widget.readTarget()) {
      // Restore the old visual route, but retain the captured attempt. Its
      // eventual commit is a stale-target no-op, never a new surface's Back.
      _motion.value = null;
    }
    _publishEligibility();
  }

  void _publishEligibility() {
    if (_notificationPending || !_current) return;
    _notificationPending = true;
    WidgetsBinding.instance
      ..addPostFrameCallback((_) {
        _notificationPending = false;
        if (!mounted || !_current) return;
        // WidgetsApp forwards this to SystemNavigator.setFrameworkHandlesBack.
        // A dirty token still handles Back (by asking), and root delegates OS.
        NavigationNotification(canHandlePop: widget.readTarget() != null)
            .dispatch(context);
      })
      ..ensureVisualUpdate();
  }

  @override
  void didChangeMetrics() => _publishEligibility();

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    // A shade/permission interruption may briefly make the app inactive while
    // it still owns the gesture. Preserve that attempt through inactive/resume.
    // Once hidden/paused/detached, abandon preview without navigating on return.
    if (state == AppLifecycleState.hidden ||
        state == AppLifecycleState.paused ||
        state == AppLifecycleState.detached) {
      _finish(committed: false);
    }
    _publishEligibility();
  }

  @override
  bool handleStartBackGesture(PredictiveBackEvent event) {
    _finish(committed: false);
    if (!_current) return false;
    final attempt = _back.begin(imeVisible: _imeVisible);
    if (attempt.owner == AndroidBackOwner.system) {
      _back.finish(attempt, committed: false);
      return false;
    }
    _attempt = attempt;
    final target = attempt.target;
    if (!event.isButtonEvent && target != null && attempt.canClose) {
      final key = widget.pageKey(target);
      if (key != null) _motion.value = _PageMotion(key, event.progress);
    }
    return true;
  }

  @override
  void handleUpdateBackGestureProgress(PredictiveBackEvent event) {
    if (!_current) {
      _finish(committed: false);
      return;
    }
    final current = _motion.value;
    if (current != null) {
      _motion.value = _PageMotion(current.key, event.progress);
    }
  }

  @override
  void handleCancelBackGesture() => _finish(committed: false);

  @override
  void handleCommitBackGesture() => _finish(committed: true);

  void _finish({required bool committed}) {
    final attempt = _attempt;
    if (attempt == null) return;
    _attempt = null;
    // End visual driving BEFORE the registry changes Pages. The route settles
    // back on cancel; a committed registry pop supplies its normal exit. Never
    // call route.handleCommitBackGesture: it pops without checking our tokens.
    _motion.value = null;
    final acceptedCommit = committed && _current;
    final owner = _back.finish(attempt, committed: acceptedCommit);
    if (acceptedCommit && owner == AndroidBackOwner.ime) {
      // Normally Android consumed the IME Back before Flutter. If it did reach
      // us, hide only the IME; the same event cannot also close a page/overlay.
      unawaited(SystemChannels.textInput.invokeMethod<void>('TextInput.hide'));
    }
  }

  @override
  Future<bool> didPopRoute() async {
    if (!_current) {
      _finish(committed: false);
      return false;
    }
    if (_attempt != null) {
      // A concurrent hardware/accessibility button is not the swipe's commit.
      // Consume it without retargeting: only that gesture's terminal callback
      // can finish it. After cancel/commit a separate button works normally.
      return true;
    }
    final attempt = _back.begin(imeVisible: _imeVisible);
    if (attempt.owner == AndroidBackOwner.system) {
      _back.finish(attempt, committed: false);
      return false;
    }
    _attempt = attempt;
    _finish(committed: true);
    return true;
  }

  @override
  void dispose() {
    _finish(committed: false);
    widget.changes.removeListener(_changed);
    WidgetsBinding.instance.removeObserver(this);
    _motion.dispose();
    if (_current) {
      // The owner is gone: leave system Back enabled even if no Navigator or
      // registry remains to send another NavigationNotification. No ancestor
      // lookup is safe during disposal. Never reset a newer host's publication.
      unawaited(SystemNavigator.setFrameworkHandlesBack(false));
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => _MotionScope(
    motion: _motion,
    // Inner Navigator notifications cannot override an open drawer or sibling
    // overlay's eligibility. Publish the registry's aggregate above instead.
    child: NotificationListener<NavigationNotification>(
      onNotification: (_) => true,
      child: widget.child,
    ),
  );
}

class _PageMotion {
  const _PageMotion(this.key, this.progress);
  final LocalKey key;
  final double progress;
}

class _MotionScope extends InheritedWidget {
  const _MotionScope({required this.motion, required super.child});
  final ValueNotifier<_PageMotion?> motion;

  @override
  bool updateShouldNotify(_MotionScope oldWidget) => oldWidget.motion != motion;
}

/// Drives only the captured Android page's transition, without registering a
/// second predictive observer. All state commits still go through PhoneBack.
class AndroidBackPageMotion extends StatefulWidget {
  const AndroidBackPageMotion({
    super.key,
    required this.pageKey,
    required this.child,
  });
  final LocalKey pageKey;
  final Widget child;

  @override
  State<AndroidBackPageMotion> createState() => _AndroidBackPageMotionState();
}

class _AndroidBackPageMotionState extends State<AndroidBackPageMotion> {
  ValueNotifier<_PageMotion?>? _motion;
  TransitionRoute<dynamic>? _driven;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final motion = context
        .dependOnInheritedWidgetOfExactType<_MotionScope>()
        ?.motion;
    if (_motion != motion) {
      _restore();
      _motion?.removeListener(_update);
      _motion = motion;
      _motion?.addListener(_update);
    }
  }

  void _update() {
    final motion = _motion?.value;
    if (motion == null ||
        motion.key != widget.pageKey ||
        MediaQuery.disableAnimationsOf(context)) {
      _restore();
      return;
    }
    final route = ModalRoute.of(context);
    if (route == null || !route.isCurrent) return;
    if (_driven == null) {
      // A new/replaced route still entering must not borrow an earlier page's
      // gesture. The shared registry also rejects its stale commit identity.
      if (route.animation?.isCompleted != true) return;
      _driven = route;
      route.handleStartBackGesture(progress: 1 - motion.progress);
    } else {
      _driven!.handleUpdateBackGestureProgress(progress: 1 - motion.progress);
    }
  }

  void _restore() {
    final route = _driven;
    _driven = null;
    // Parent teardown can dispose the Navigator/route without a registry
    // notification. Never drive a controller whose owning route has gone.
    final navigator = route?.navigator;
    if (route != null && navigator != null && navigator.mounted) {
      route.handleCancelBackGesture();
    }
  }

  @override
  void dispose() {
    _restore();
    _motion?.removeListener(_update);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
