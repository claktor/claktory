import 'dart:convert';
import 'dart:developer' as developer;

import 'package:flutter/foundation.dart';

import 'transport.dart';

/// One app view's surface as this app holds it: its state, its A2UI
/// components by id and its data model, which inputs write to here (two-way
/// binding, local to this device until an action sends it).
class AppSurfaceModel extends ChangeNotifier {
  AppSurfaceModel(AppSurfaceData surface) : surfaceId = surface.surfaceId {
    replace(surface);
  }

  final String surfaceId;

  AppViewState get state => _state;
  var _state = AppViewState.opening;

  /// Why it's failed, stopped or closed; empty otherwise.
  String get error => _error;
  var _error = '';

  String get title => _title;
  var _title = '';

  String get catalog => _catalog;
  var _catalog = appViewCatalogV1;

  /// The last update applied.
  int get seq => _seq;
  var _seq = 0;

  /// Its components by id, as A2UI JSON objects.
  Map<String, Map<String, Object?>> get components => _components;
  var _components = <String, Map<String, Object?>>{};

  /// Its data model's root.
  Object? get data => _data;
  Object? _data = <String, Object?>{};

  /// Whether there's anything to show: the app made its root.
  bool get hasContent => _components.containsKey('root');

  /// Takes [surface] in place of everything held (AppSurface).
  void replace(AppSurfaceData surface) {
    _state = surface.state;
    _error = surface.error;
    _title = surface.title;
    _catalog = surface.catalog;
    _seq = surface.seq;
    final created = _decode(surface.snapshot)?['createSurface'];
    if (created is Map<String, Object?>) {
      _data = _copy(created['dataModel']) ?? <String, Object?>{};
      _components = {};
      _upsert(created['components']);
    } else if (surface.snapshot.isEmpty &&
        surface.state != AppViewState.failed &&
        surface.state != AppViewState.closed) {
      // Not made yet (opening, stopped): nothing to show.
      _components = {};
      _data = <String, Object?>{};
    }
    notifyListeners();
  }

  /// Applies [update] when it's the next one; one already applied is
  /// ignored. False when one was skipped: the surface must be opened again
  /// for a fresh snapshot.
  bool apply(AppViewUpdateData update) {
    if (update.seq <= _seq) return true;
    if (update.seq != _seq + 1) return false;
    _seq = update.seq;
    final message = _decode(update.message);
    if (message == null) return true;
    if (message['updateComponents'] case final Map<String, Object?> body) {
      _upsert(body['components']);
    } else if (message['updateDataModel']
        case final Map<String, Object?> body) {
      final path = body['path'];
      _data = setAtPointer(
        _data,
        path is String ? path : '/',
        _copy(body['value']),
      );
    } else if (message.containsKey('deleteSurface')) {
      _components = {};
      _data = <String, Object?>{};
      _state = AppViewState.closed;
    }
    notifyListeners();
    return true;
  }

  /// What's at [pointer] in the data model; null when nothing is.
  Object? read(String pointer) => readAtPointer(_data, pointer);

  /// An input's change: written here only, until an action sends it.
  void write(String pointer, Object? value) {
    _data = setAtPointer(_data, pointer, value);
    notifyListeners();
  }

  void _upsert(Object? components) {
    if (components is! List) return;
    for (final component in components) {
      if (component is Map<String, Object?> && component['id'] is String) {
        _components[component['id']! as String] = component;
      }
    }
  }
}

Map<String, Object?>? _decode(String json) {
  if (json.isEmpty) return null;
  try {
    final decoded = jsonDecode(json);
    return decoded is Map<String, Object?> ? decoded : null;
  } on FormatException catch (error) {
    // The backend checks every message; one that still can't be read is
    // left out rather than taking the view down.
    developer.log('unreadable app view message: $error', name: 'app_views');
    return null;
  }
}

/// A deep copy of decoded JSON, so changes here never reach what was sent.
Object? _copy(Object? value) => switch (value) {
  final Map<String, Object?> map => {
    for (final entry in map.entries) entry.key: _copy(entry.value),
  },
  final List<Object?> list => [for (final item in list) _copy(item)],
  _ => value,
};

/// [pointer]'s steps (RFC 6901): "/a/b~1c" is a, b/c. The root ("" or
/// "/") has none.
List<String> pointerSteps(String pointer) {
  if (pointer.isEmpty || pointer == '/') return const [];
  final body = pointer.startsWith('/') ? pointer.substring(1) : pointer;
  return [
    for (final step in body.split('/'))
      step.replaceAll('~1', '/').replaceAll('~0', '~'),
  ];
}

/// [step] made safe inside a pointer.
String pointerStep(String step) =>
    step.replaceAll('~', '~0').replaceAll('/', '~1');

/// What's at [pointer] in [root]; null when nothing is.
Object? readAtPointer(Object? root, String pointer) {
  var at = root;
  for (final step in pointerSteps(pointer)) {
    if (at is Map<String, Object?>) {
      at = at[step];
    } else if (at is List<Object?>) {
      final index = int.tryParse(step);
      if (index == null || index < 0 || index >= at.length) return null;
      at = at[index];
    } else {
      return null;
    }
  }
  return at;
}

/// [root] with [value] at [pointer], making the objects on the way; a null
/// [value] deletes what's there. At the root, [value] replaces it all.
Object? setAtPointer(Object? root, String pointer, Object? value) {
  final steps = pointerSteps(pointer);
  if (steps.isEmpty) return value ?? <String, Object?>{};
  final top = root is Map<String, Object?> || root is List<Object?>
      ? root
      : <String, Object?>{};
  Object? parent = top;
  for (var i = 0; i < steps.length; i++) {
    final step = steps[i];
    final last = i == steps.length - 1;
    if (parent is Map<String, Object?>) {
      if (last) {
        if (value == null) {
          parent.remove(step);
        } else {
          parent[step] = value;
        }
        break;
      }
      final next = parent[step];
      if (next is Map<String, Object?> || next is List<Object?>) {
        parent = next;
      } else {
        parent = parent[step] = <String, Object?>{};
      }
    } else if (parent is List<Object?>) {
      final index = step == '-' ? parent.length : int.tryParse(step);
      if (index == null || index < 0 || index > parent.length) break;
      if (last) {
        if (value == null) {
          if (index < parent.length) parent.removeAt(index);
        } else if (index == parent.length) {
          parent.add(value);
        } else {
          parent[index] = value;
        }
        break;
      }
      if (index == parent.length) parent.add(<String, Object?>{});
      final next = parent[index];
      if (next is Map<String, Object?> || next is List<Object?>) {
        parent = next;
      } else {
        parent = parent[index] = <String, Object?>{};
      }
    } else {
      break;
    }
  }
  return top;
}
