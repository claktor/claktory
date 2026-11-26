import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import '../backend.dart';

/// One request to open a shared website. A late failure from an older
/// request must not release the Claktory serving its replacement.
class AndroidShareBrowserOpen {
  AndroidShareBrowserOpen._(this._origin);

  final Object _origin;
}

/// Called immediately before the Android browser's `open` method. Only
/// this Claktory, not every saved Claktory, serves the Custom Tab.
/// Registration alone never keeps a connection alive: the native binding
/// must also report that the tab is visible.
AndroidShareBrowserOpen shareBrowserOpening(Backend backend) =>
    defaultTargetPlatform == TargetPlatform.android
    ? androidShareBrowserLifecycle.opening(backend)
    : AndroidShareBrowserOpen._(backend);

/// Also called for the native `browser` fallback: an ordinary browser
/// does not keep Claktory's process up. The matching request alone clears.
void shareBrowserOpenFailed(AndroidShareBrowserOpen opening) {
  if (defaultTargetPlatform == TargetPlatform.android) {
    androidShareBrowserLifecycle.openFailed(opening);
  }
}

/// One process-wide native stream, shared by the per-Claktory adapters.
final androidShareBrowserLifecycle = AndroidShareBrowserLifecycle();

/// Transient presentation state, not a saved share or a second outbox.
/// The native stream says only whether its Custom Tab binding is alive;
/// [opening] supplies the originating Claktory without persisting it.
/// An initial native `true` with no known origin protects no connection.
class AndroidShareBrowserLifecycle extends ChangeNotifier {
  AndroidShareBrowserLifecycle({
    this.channel = const EventChannel('factory/share-browser/events'),
  });

  final EventChannel channel;
  StreamSubscription<Object?>? _subscription;
  AndroidShareBrowserOpen? _opening;
  var _visible = false;
  var _watchers = 0;
  var _generation = 0;
  var _disposed = false;

  bool visibleFor(Object origin) =>
      !_disposed && _visible && identical(_opening?._origin, origin);

  AndroidShareBrowserOpen opening(Object origin) {
    final request = AndroidShareBrowserOpen._(origin);
    if (_disposed) return request;
    _opening = request;
    notifyListeners();
    return request;
  }

  void openFailed(AndroidShareBrowserOpen request) {
    if (_disposed || !identical(_opening, request)) return;
    _opening = null;
    notifyListeners();
  }

  /// A Claktory was removed while another one still watches the stream.
  void forget(Object origin) {
    if (_disposed || !identical(_opening?._origin, origin)) return;
    _opening = null;
    notifyListeners();
  }

  /// Listen once regardless of the number of Backends. The returned
  /// cancellation is idempotent; the final one releases the native stream
  /// and the origin, so a removed Claktory is never held by this adapter.
  VoidCallback watch(VoidCallback changed) {
    if (_disposed) return () {};
    addListener(changed);
    if (++_watchers == 1) _listen();
    var cancelled = false;
    return () {
      if (cancelled || _disposed) return;
      cancelled = true;
      removeListener(changed);
      if (--_watchers == 0) _stop();
    };
  }

  void _listen() {
    final generation = ++_generation;
    _subscription = channel.receiveBroadcastStream().listen(
      (event) {
        if (_disposed || generation != _generation) return;
        final visible = event is Map && event['visible'] == true;
        if (_visible == visible) return;
        _visible = visible;
        notifyListeners();
      },
      onError: (Object _) {
        if (_disposed || generation != _generation) return;
        // Missing/failed native capability never grants an unbounded
        // background connection. Its ordinary grace applies instead.
        _visible = false;
        _opening = null;
        notifyListeners();
      },
      onDone: () {
        if (_disposed || generation != _generation) return;
        _visible = false;
        _opening = null;
        notifyListeners();
      },
    );
  }

  void _stop() {
    _generation++;
    unawaited(_subscription?.cancel());
    _subscription = null;
    _opening = null;
    _visible = false;
  }

  @override
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _stop();
    super.dispose();
  }
}
