import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import '../backend.dart';
import 'background_policy.dart';
import 'share_browser_lifecycle.dart';

/// Binds Android's short background grace to the shared transport. The
/// existing app lifecycle calls hidden/resumed on every Backend; a newly
/// created connection also checks whether the app is already hidden.
class AndroidBackgroundConnection {
  factory AndroidBackgroundConnection(
    Backend backend, {
    AndroidShareBrowserLifecycle? browser,
  }) {
    final work = _BackgroundWork(backend);
    final state = WidgetsBinding.instance.lifecycleState;
    if (state == AppLifecycleState.hidden ||
        state == AppLifecycleState.paused) {
      // run() is still constructing us, so Backend's adapter field is
      // null here. Its timestamp must agree with our initial grace timer.
      backend.hidden();
    }
    return AndroidBackgroundConnection._(
      AndroidBackgroundPolicy(busy: work, suspend: backend.suspend),
      work,
      backend,
      browser ??
          (defaultTargetPlatform == TargetPlatform.android
              ? androidShareBrowserLifecycle
              : null),
    );
  }

  AndroidBackgroundConnection._(
    this._policy,
    this._work,
    this._backend,
    this._browser,
  ) {
    _unwatch = _browser?.watch(_browserChanged);
    _browserChanged();
    final state = WidgetsBinding.instance.lifecycleState;
    if (state == AppLifecycleState.hidden ||
        state == AppLifecycleState.paused) {
      hidden();
    }
  }

  @visibleForTesting
  AndroidBackgroundConnection.forTesting({
    required ValueListenable<bool> busy,
    required VoidCallback suspend,
  }) : _policy = AndroidBackgroundPolicy(busy: busy, suspend: suspend),
       _work = null,
       _backend = null,
       _browser = null;

  final AndroidBackgroundPolicy _policy;
  final _BackgroundWork? _work;
  final Backend? _backend;
  final AndroidShareBrowserLifecycle? _browser;
  VoidCallback? _unwatch;
  var _disposed = false;

  void _browserChanged() {
    if (_disposed) return;
    final backend = _backend;
    if (backend == null) return;
    final visible = _browser?.visibleFor(backend) ?? false;
    _policy.externalVisible(visible);
    backend.keepTryingWhileHidden = visible;
  }

  void hidden() => _policy.hidden();

  void resumed() => _policy.resumed();

  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _unwatch?.call();
    final backend = _backend;
    if (backend != null) {
      _browser?.forget(backend);
      backend.keepTryingWhileHidden = false;
    }
    _policy.dispose();
    _work?.dispose();
  }
}

/// The whole send, not individual chunk requests: async file reads,
/// uploads, transcription, and the message's acknowledgement all count.
/// Failed/connection-waiting outbox messages and unsent recordings don't.
/// Pings, diagnostics and incoming conversation streaming never count.
class _BackgroundWork extends ChangeNotifier implements ValueListenable<bool> {
  _BackgroundWork(this.backend)
    : _changes = Listenable.merge([
        backend.holds,
        backend.outbox,
        backend.chatOutbox,
      ]) {
    _changes.addListener(_changed);
  }

  final Backend backend;
  final Listenable _changes;

  @override
  bool get value =>
      backend.holds.value ||
      backend.outbox.sending ||
      backend.chatOutbox.sending;

  void _changed() => notifyListeners();

  @override
  void dispose() {
    _changes.removeListener(_changed);
    super.dispose();
  }
}
