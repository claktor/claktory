import 'dart:async';

import 'package:flutter/foundation.dart';

import '../background_grace.dart';

/// Android has no push yet. Keep a live connection through a short trip
/// away, then let its transport go quiet. A send already on its way may
/// finish first; a failed or queued send is not work keeping it alive.
///
/// This owns only the timing policy. The shared Backend owns suspending
/// its transport and resuming its stores and revisions, without losing
/// drafts or outbox messages. No service or wake lock keeps Android alive.
class AndroidBackgroundPolicy {
  AndroidBackgroundPolicy({
    required this.busy,
    required this.suspend,
    this.grace = BackgroundGrace.length,
  }) {
    busy.addListener(_workChanged);
  }

  final ValueListenable<bool> busy;
  final VoidCallback suspend;
  final Duration grace;

  Timer? _grace;
  Timer? _settled;
  var _hidden = false;
  var _expired = false;
  var _suspended = false;
  var _disposed = false;

  /// Hidden or paused, not merely inactive (a permission prompt or the
  /// notification shade can cover the app without taking it away).
  void hidden() {
    if (_disposed || _hidden) return;
    _hidden = true;
    _expired = false;
    _suspended = false;
    _grace = Timer(grace, () {
      _grace = null;
      _expired = true;
      _workChanged();
    });
  }

  /// Back in sight: cancel a stop even if it was queued for this turn.
  /// The caller resumes the Backend through its existing resume path.
  void resumed() {
    if (_disposed) return;
    _hidden = false;
    _expired = false;
    _suspended = false;
    _grace?.cancel();
    _grace = null;
    _settled?.cancel();
    _settled = null;
  }

  void _workChanged() {
    _settled?.cancel();
    _settled = null;
    if (_disposed || !_hidden || !_expired || _suspended || busy.value) {
      return;
    }
    // Let a synchronous/microtask handoff (recording -> upload -> send)
    // settle before deciding it finished. Busy must cover the *whole*
    // operation, including async file reads between upload chunks.
    _settled = Timer(Duration.zero, () {
      _settled = null;
      if (_disposed || !_hidden || !_expired || _suspended || busy.value) {
        return;
      }
      _suspended = true;
      suspend();
    });
  }

  /// The Backend went away permanently; never suspend a replacement one.
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _grace?.cancel();
    _settled?.cancel();
    busy.removeListener(_workChanged);
  }
}
