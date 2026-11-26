// Whether anyone can see the window, and whether the person asked for
// less motion, as the Mac tells it (MainFlutterWindow.swift): Flutter's
// macOS embedder reports neither a covered window nor Reduce motion. The
// Windows runner tells the same (app/windows/runner/sight_channel.h): its
// embedder passes on no Animation effects setting, and its lifecycle misses
// a locked session, a dark display and another virtual desktop. On other
// platforms the app's lifecycle and the platform's own Reduce motion
// already say so, and this changes nothing.
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

/// What the Mac says about the window: [shown] while any of it is on
/// screen (not covered by other windows, minimized or hidden), and
/// [reduceMotion] while System Settings → Accessibility → Display → Reduce
/// motion is on. On Windows, Settings → Accessibility → Visual effects →
/// Animation effects off is [reduceMotion].
class Sight {
  Sight._();

  static const _channel = MethodChannel('factory/sight');

  static final shown = ValueNotifier(true);
  static final reduceMotion = ValueNotifier(false);

  static var _started = false;

  /// Asks the Mac or Windows once and listens for changes after; nothing
  /// elsewhere.
  static void start() {
    if (_started ||
        kIsWeb ||
        (defaultTargetPlatform != TargetPlatform.macOS &&
            defaultTargetPlatform != TargetPlatform.windows)) {
      return;
    }
    _started = true;
    _channel.setMethodCallHandler((call) async {
      if (call.method == 'sight') _take(call.arguments);
    });
    _channel
        .invokeMethod<Object?>('state')
        .then(_take)
        .catchError((Object _) {});
  }

  /// Back to how it starts, for tests.
  @visibleForTesting
  static void debugReset() {
    _started = false;
    shown.value = true;
    reduceMotion.value = false;
    _channel.setMethodCallHandler(null);
  }

  /// The person's pointer is over the window, so it's on screen, whatever
  /// the Mac last said: its report goes stale (flutter#155977), and a
  /// window uncovered while another app is in front can stay "unseen"
  /// until it's clicked (friend bugs #31 and #40). The Mac is told, so it
  /// says so again once the window really is covered.
  static void pointed() {
    if (shown.value) return;
    shown.value = true;
    if (!_started) return;
    _channel.invokeMethod<Object?>('seen').catchError((Object _) => null);
  }

  static void _take(Object? state) {
    if (state is! Map) return;
    if (state['shown'] case final bool value) shown.value = value;
    if (state['reduceMotion'] case final bool value) reduceMotion.value = value;
  }
}

/// Stops everything below that moves by itself while no one can see the
/// window ([TickerMode], which the working waves, the characters, the caret
/// and every animation follow), and passes the Mac's Reduce motion on as
/// [MediaQueryData.disableAnimations]. The pointer moving over the window
/// counts as seen ([Sight.pointed]).
class SightScope extends StatelessWidget {
  const SightScope({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([Sight.shown, Sight.reduceMotion]),
    builder: (context, child) {
      final media = MediaQuery.of(context);
      return TickerMode(
        enabled: Sight.shown.value,
        child: MediaQuery(
          data: media.copyWith(
            disableAnimations:
                media.disableAnimations || Sight.reduceMotion.value,
          ),
          child: child!,
        ),
      );
    },
    // Seen by the pointer, never taking it: everything below gets it as
    // before.
    child: Listener(
      behavior: HitTestBehavior.translucent,
      onPointerHover: (_) => Sight.pointed(),
      onPointerDown: (_) => Sight.pointed(),
      onPointerSignal: (_) => Sight.pointed(),
      child: child,
    ),
  );
}
