import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';

import '../backend.dart' show RequestFailure;
import '../protocol/factory/v1/factory.pb.dart' show ErrorCode;
import 'surface.dart';
import 'transport.dart';

/// One app view shown in a pane or on a screen: opens it, follows the
/// backend's changes to it in order (opening it again for a fresh snapshot
/// when one was missed, and after reconnecting), sends what the person
/// does, and closes it when it goes.
class AppViewController extends ChangeNotifier {
  AppViewController({
    required this.transport,
    required this.appId,
    this.view = '',
    this.params = '',
  }) {
    _events = transport.viewEvents.listen(_event);
    transport.viewsConnected.addListener(_connectionChanged);
    _wasConnected = transport.viewsConnected.value;
    if (_wasConnected) unawaited(open());
  }

  final AppViewTransport transport;
  final String appId;

  /// The view (AppViewSpec.id; empty for the app's first).
  final String view;

  /// Its params, a JSON object, or empty.
  final String params;

  /// The surface shown; null before the backend answered.
  AppSurfaceModel? get surface => _surface;
  AppSurfaceModel? _surface;

  /// Why it couldn't be opened; null when it was, or while it opens.
  String? get failure => _failure;
  String? _failure;

  /// Opening it now.
  bool get opening => _opening;
  var _opening = false;

  late final StreamSubscription<AppViewEvent> _events;
  var _wasConnected = false;
  var _disposed = false;

  /// What came for a surface before its OpenAppView was answered, applied
  /// once it is.
  final _early = <AppViewEvent>[];

  /// Opens it, or again: a fresh snapshot in place of what's held.
  Future<void> open() async {
    if (_disposed) return;
    _opening = true;
    _failure = null;
    notifyListeners();
    final AppSurfaceData answer;
    try {
      answer = await transport.openView(
        appId: appId,
        view: view,
        params: params,
        catalogs: appViewCatalogs,
      );
    } on RequestFailure catch (failure) {
      if (_disposed) return;
      _opening = false;
      _failure = failure.message;
      notifyListeners();
      return;
    }
    if (_disposed) {
      unawaited(_close(answer.surfaceId));
      return;
    }
    _opening = false;
    final old = _surface;
    if (old != null && old.surfaceId == answer.surfaceId) {
      old.replace(answer);
    } else {
      if (old != null) {
        old.dispose();
        // A new surface in its place: the old one isn't shown any more.
        unawaited(_close(old.surfaceId));
      }
      _surface = AppSurfaceModel(answer);
    }
    final early = [
      for (final event in _early)
        if (event.surfaceId == answer.surfaceId) event,
    ];
    _early.clear();
    notifyListeners();
    for (final event in early) {
      _event(event);
    }
  }

  void _event(AppViewEvent event) {
    final surface = _surface;
    if (surface == null || surface.surfaceId != event.surfaceId) {
      if (_opening) _early.add(event);
      return;
    }
    switch (event) {
      case AppSurfaceChanged(:final surface):
        _surface!.replace(surface);
      case AppSurfaceUpdated(:final update):
        if (!surface.apply(update)) unawaited(open());
    }
  }

  void _connectionChanged() {
    final connected = transport.viewsConnected.value;
    // Back after a drop: surfaces live only in the backend's memory.
    if (connected && !_wasConnected) unawaited(open());
    _wasConnected = connected;
  }

  /// The person used [componentId] (its action [event], A2UI's
  /// `action.event`), in the data scope [scope]: its context resolved
  /// against the data model, sent to the app. A surface that's gone is
  /// opened again.
  Future<void> act(
    String componentId,
    Map<String, Object?> event,
    String scope,
  ) async {
    final surface = _surface;
    final name = event['name'];
    if (surface == null || name is! String) return;
    final context = <String, Object?>{};
    if (event['context'] case final Map<String, Object?> asked) {
      for (final entry in asked.entries) {
        context[entry.key] = resolveValue(surface, entry.value, scope);
      }
    }
    final message = jsonEncode({
      'version': '1.0',
      'action': {
        'name': name,
        'surfaceId': surface.surfaceId,
        'sourceComponentId': componentId,
        'timestamp': DateTime.now().toUtc().toIso8601String(),
        'context': context,
      },
    });
    try {
      await transport.actInView(surface.surfaceId, message);
    } on RequestFailure catch (failure) {
      if (failure.code == ErrorCode.ERROR_CODE_NOT_FOUND) {
        await open();
      } else if (!_disposed) {
        _failure = failure.message;
        notifyListeners();
      }
    }
  }

  Future<void> _close(String surfaceId) async {
    try {
      await transport.closeView(surfaceId);
    } on RequestFailure {
      // Gone already.
    }
  }

  @override
  void dispose() {
    _disposed = true;
    unawaited(_events.cancel());
    transport.viewsConnected.removeListener(_connectionChanged);
    final surface = _surface;
    if (surface != null) {
      unawaited(_close(surface.surfaceId));
      surface.dispose();
    }
    super.dispose();
  }
}

/// [pointer] against the data [scope] a list template made: absolute
/// ("/a") as it is, relative ("a") under the scope.
String scopedPointer(String pointer, String scope) {
  if (pointer.startsWith('/')) return pointer;
  if (pointer.isEmpty) return scope.isEmpty ? '/' : scope;
  return '$scope/$pointer';
}

/// A dynamic value (a literal, or `{"@path": …}`) as it is now in
/// [surface], in [scope].
Object? resolveValue(AppSurfaceModel surface, Object? value, String scope) {
  if (value is Map<String, Object?> && value['@path'] is String) {
    return surface.read(scopedPointer(value['@path']! as String, scope));
  }
  return value;
}
