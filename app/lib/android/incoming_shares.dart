import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// An incoming Android share, copied and bounded by factory/shared before it
/// reaches Dart. Files are native attachment-cache paths, never provider URIs.
/// This is transient intake, not a second draft store or a send operation.
class AndroidIncomingShare {
  const AndroidIncomingShare({
    required this.id,
    required this.text,
    required this.subject,
    required this.files,
    required this.error,
    required this.textCut,
  });

  final int id;
  final String? text;
  final String? subject;
  final List<String> files;
  final String? error;
  final bool textCut;

  /// Keep a shared title as well as its text without repeating identical text.
  String get draftText => [
    if (subject case final subject? when subject.isNotEmpty && subject != text)
      subject,
    if (text case final text? when text.isNotEmpty) text,
  ].join('\n');

  bool get hasContent => draftText.isNotEmpty || files.isNotEmpty;
}

/// Malformed envelopes cannot become provider-URI attachments or crash the
/// window. Native limits are checked again; unknown error codes still
/// reach presentation as a generic failure rather than hiding the share.
AndroidIncomingShare? parseAndroidIncomingShare(Object? value) {
  if (value is! Map) return null;
  final id = value['id'];
  final text = value['text'];
  final subject = value['subject'];
  final files = value['files'];
  final error = value['error'];
  final cut = value['text_cut'];
  if (id is! int || id <= 0 || id > 0x7fffffff || cut is! bool) return null;
  if (text != null && (text is! String || text.length > 100000)) return null;
  if (subject != null && (subject is! String || subject.length > 1000)) {
    return null;
  }
  if (error != null &&
      (error is! String || error.isEmpty || error.length > 100)) {
    return null;
  }
  if (files is! List || files.length > 20) return null;
  final paths = <String>[];
  for (final file in files) {
    if (file is! String ||
        !file.startsWith('/') ||
        file.length > 4096 ||
        file.contains('\u0000')) {
      return null;
    }
    if (!paths.contains(file)) paths.add(file);
  }
  final share = AndroidIncomingShare(
    id: id,
    text: text as String?,
    subject: subject as String?,
    files: List.unmodifiable(paths),
    error: error as String?,
    textCut: cut,
  );
  return share.hasContent || error != null ? share : null;
}

/// One listener for the window's life. Native cold intake and warm callbacks
/// can cross: install the callback first, merge by process-monotonic id, and
/// expose the sorted initial queue only once listen has finished.
///
/// [claim] transfers cache-file ownership to the existing attachment draft
/// flow BEFORE selecting another Claktory can retire the picker. Claiming never
/// sends. [discard] alone asks native to delete unaccepted copies. Nothing is
/// persisted; accepted attachments have the same lifetime as ordinary pickers.
class AndroidIncomingShares extends ChangeNotifier {
  AndroidIncomingShares({
    this._channel = const MethodChannel('factory/shared'),
  });

  final MethodChannel _channel;
  final _pending = <AndroidIncomingShare>[];
  final _seen = <int>{};
  final _claimed = <int>{};
  Future<void>? _started;
  bool _ready = false;
  bool _disposed = false;

  List<AndroidIncomingShare> get pending => List.unmodifiable(_pending);
  bool get ready => _ready;

  Future<void> start() => _started ??= _start();

  Future<void> _start() async {
    if (_disposed) return;
    _channel.setMethodCallHandler((call) async {
      if (!_disposed && call.method == 'received') {
        final changed = _merge(call.arguments);
        if (changed && _ready) notifyListeners();
      }
    });
    try {
      final initial = await _channel.invokeMethod<Object?>('listen');
      if (initial is List) {
        for (final value in initial) {
          if (_disposed) {
            // A cold read completed after window disposal. Those shares were
            // never accepted, so clean up their copies as well.
            final share = parseAndroidIncomingShare(value);
            if (share != null && _seen.add(share.id)) {
              unawaited(_discardNative(share.id));
            }
          } else {
            _merge(value);
          }
        }
      }
    } on MissingPluginException {
      // Older APKs and non-native test embeddings have no incoming shares.
    } on PlatformException {
      // No envelope was delivered. Do not invent a successful share.
    }
    if (_disposed) return;
    _ready = true;
    notifyListeners();
  }

  bool _merge(Object? value) {
    final share = parseAndroidIncomingShare(value);
    if (share == null || !_seen.add(share.id)) return false;
    _pending.add(share);
    _pending.sort((a, b) => a.id.compareTo(b.id));
    return true;
  }

  /// Removes an accepted share without deleting its files. Call before any
  /// asynchronous transfer or navigation, so a disappearing picker cannot
  /// discard the files now going to a draft. Each envelope is claimable once.
  AndroidIncomingShare? claim(int id) {
    if (_disposed || !_ready) return null;
    final index = _pending.indexWhere((share) => share.id == id);
    if (index < 0) return null;
    final share = _pending.removeAt(index);
    _claimed.add(id);
    notifyListeners();
    return share;
  }

  /// The chosen conversation vanished during staging. Keep the same copies
  /// for another choice, without treating a duplicate native callback as new.
  void retryClaimed(AndroidIncomingShare share) {
    if (_disposed || !_claimed.remove(share.id)) return;
    _pending.add(share);
    _pending.sort((a, b) => a.id.compareTo(b.id));
    notifyListeners();
  }

  Future<void> discard(int id) async {
    if (_disposed) return;
    final index = _pending.indexWhere((share) => share.id == id);
    if (index < 0) return;
    _pending.removeAt(index);
    notifyListeners();
    await _discardNative(id);
  }

  Future<void> _discardNative(int id) async {
    try {
      await _channel.invokeMethod<void>('discard', {'id': id});
    } on MissingPluginException {
      // Copies, if any, also have the native cache's normal expiry.
    } on PlatformException {
      // Cancellation must still finish if native cleanup cannot run.
    }
  }

  @override
  void dispose() {
    _disposed = true;
    _channel.setMethodCallHandler(null);
    for (final share in _pending) {
      unawaited(_discardNative(share.id));
    }
    _pending.clear();
    super.dispose();
  }
}
