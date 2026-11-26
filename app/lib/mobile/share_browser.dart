import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// The page of a website a friend shares over a pipe, open over the app on
/// an iPhone or iPad (W6, #505: ios/Runner/ShareBrowser.swift, Safari's own
/// view). The app stays in front while it's open, so its connection and the
/// share's forward stay up by themselves; what it needs to know is which
/// share's page is on screen, from which Claktory, to forward it again
/// should its tunnel be made afresh meanwhile (back from the Home Screen),
/// and whether the page's first load failed, to say so plainly once the
/// person is back (Done).
///
/// Only what's on screen now, in memory: nothing is kept on the device.
class InAppShareBrowser extends ChangeNotifier {
  InAppShareBrowser({
    this.channel = const EventChannel('factory/share-browser/events'),
  });

  final EventChannel channel;
  StreamSubscription<Object?>? _subscription;

  /// The share whose page was opened last, and the Claktory it's from
  /// (its Backend), until its page closes.
  ({Object origin, String shareId})? _opened;
  var _visible = false;
  var _failed = false;

  /// Whether [shareId]'s page, opened from [origin], is on screen.
  bool shows(Object origin, String shareId) =>
      _visible &&
      identical(_opened?.origin, origin) &&
      _opened?.shareId == shareId;

  /// Whether a share's page from [origin] is on screen.
  bool showsFrom(Object origin) =>
      _visible && identical(_opened?.origin, origin);

  /// [shareId]'s page, opened from [origin], is about to show: called just
  /// before the channel's `open`.
  void opening(Object origin, String shareId) {
    _listen();
    _opened = (origin: origin, shareId: shareId);
    _failed = false;
    notifyListeners();
  }

  /// The page didn't open after all.
  void openFailed(Object origin, String shareId) {
    if (!identical(_opened?.origin, origin) || _opened?.shareId != shareId) {
      return;
    }
    _opened = null;
    _visible = false;
    notifyListeners();
  }

  /// The person is back from [shareId]'s page (Done) and its first load
  /// had failed: true once, then false, so it's said once.
  bool takeFailure(Object origin, String shareId) {
    if (_visible ||
        !_failed ||
        !identical(_closed?.origin, origin) ||
        _closed?.shareId != shareId) {
      return false;
    }
    _failed = false;
    return true;
  }

  /// The share whose page closed last, for [takeFailure].
  ({Object origin, String shareId})? _closed;

  /// A Claktory was removed: its page, if any, is forgotten.
  void forget(Object origin) {
    if (identical(_opened?.origin, origin)) _opened = null;
    if (identical(_closed?.origin, origin)) _closed = null;
    notifyListeners();
  }

  void _listen() {
    if (_subscription != null || defaultTargetPlatform != TargetPlatform.iOS) {
      return;
    }
    _subscription = channel.receiveBroadcastStream().listen(
      _event,
      onError: (Object _) => _gone(),
      onDone: _gone,
    );
  }

  void _event(Object? event) {
    if (event is! Map) return;
    final visible = event['visible'] == true;
    if (event['loaded'] == false) _failed = true;
    if (visible == _visible && event['loaded'] != false) return;
    if (!visible && _visible) {
      _closed = _opened;
      _opened = null;
    }
    _visible = visible;
    notifyListeners();
  }

  void _gone() {
    _subscription = null;
    _visible = false;
    _opened = null;
    notifyListeners();
  }

  /// For tests: what the native side would say.
  @visibleForTesting
  void debugEvent(Map<String, Object?> event) => _event(event);

  /// For tests: as it was at launch, not listening.
  @visibleForTesting
  void debugReset() {
    unawaited(_subscription?.cancel());
    _subscription = null;
    _opened = null;
    _closed = null;
    _visible = false;
    _failed = false;
  }

  @override
  void dispose() {
    unawaited(_subscription?.cancel());
    _subscription = null;
    super.dispose();
  }
}

/// The one page over the app, for every Claktory.
final inAppShareBrowser = InAppShareBrowser();
