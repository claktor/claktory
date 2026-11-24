// Recordings kept on this device until their Claktory has them (#401,
// Matt's decision: an unsent recording stays on the device until the
// Claktory confirms it got it, then it's deleted). A voice message to an
// agent or a chat, or a field's dictation, is recorded straight into a
// private folder (Recorder.swift's voiceFolder: never backed up, this
// user's only; Android's no_backup folder), with what it's for beside it
// (kept.json), so a dropped connection, the app quit or killed, loses
// nothing: on the next launch it comes back where it belongs and is sent
// again, with the same ids, so nothing doubles.
import 'dart:async';
import 'dart:convert';
import 'dart:developer' as developer;
import 'dart:io';
import 'dart:math';

import 'package:crypto/crypto.dart' as crypto;
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'backend.dart';
import 'connections.dart' show RemoteFactory;
import 'recorder.dart';

/// What a kept recording is for.
enum KeptKind {
  /// A voice message to an agent ([KeptVoice.target], its id).
  agent,

  /// A voice message to a chat ([KeptVoice.target], its id).
  chat,

  /// A field's dictation ([KeptVoice.target], the field's key).
  dictation,
}

final _random = Random.secure();

/// A new id for an upload, a request or a message.
String newVoiceId() =>
    List.generate(16, (_) => _random.nextInt(16).toRadixString(16)).join();

/// A recording not yet confirmed by its Claktory, and what it's for.
class KeptVoice extends Recording {
  KeptVoice({
    required super.path,
    required super.duration,
    required this.claktory,
    required this.kind,
    this.target = '',
    this.label = '',
    DateTime? made,
    this.clientId = '',
    String? uploadId,
    String? requestId,
    this.uploaded = false,
    this.kept = true,
  }) : made = made ?? DateTime.now(),
       uploadId = uploadId ?? newVoiceId(),
       requestId = requestId ?? newVoiceId();

  /// Its Claktory ([keptClaktoryKey]).
  final String claktory;
  final KeptKind kind;

  /// The agent's or chat's id, or the dictation's field's key (empty for a
  /// field that has none: it isn't put back in one after a relaunch).
  final String target;

  /// What it was for, in the person's words ("Your answer to June"), for
  /// a dictation whose field has gone.
  final String label;
  final DateTime made;

  /// A voice message's client id: its repeat is the same message.
  final String clientId;

  /// Its upload's id: a repeat picks up what the Claktory has of it.
  String uploadId;

  /// A dictation's TranscribeAudio id: a repeat attaches to the job, or
  /// gets its answer.
  String requestId;

  /// A dictation's upload finished: it's asked for the words only.
  bool uploaded;

  /// Kept on disk: false where the device has no folder for it (tests),
  /// when it lives only as long as the app does.
  final bool kept;

  /// The Claktory no longer has the recording (no_recording): it goes up
  /// again, as a new upload.
  void renew() {
    uploadId = newVoiceId();
    requestId = newVoiceId();
    uploaded = false;
  }

  /// Its folder: the recording and kept.json.
  Directory get folder => File(path).parent;

  File get _record => File('${folder.path}/$recordName');

  /// The file beside a recording saying what it's for.
  static const recordName = 'kept.json';

  /// Writes what it's for beside it, replacing what was there.
  Future<void> save() async {
    if (!kept) return;
    try {
      final partial = File('${_record.path}.partial');
      await partial.writeAsString(jsonEncode(toJson()), flush: true);
      await partial.rename(_record.path);
    } on FileSystemException catch (error) {
      developer.log('could not keep a recording: $error', name: 'voice');
    }
  }

  /// Deletes it from the device: its Claktory has it, or the person
  /// deleted it.
  Future<void> delete() async {
    try {
      final folder = this.folder;
      if (!kept) {
        final file = File(path);
        if (file.existsSync()) await file.delete();
        return;
      }
      if (folder.existsSync()) await folder.delete(recursive: true);
    } on FileSystemException catch (error) {
      developer.log('could not delete a recording: $error', name: 'voice');
    }
  }

  Map<String, Object?> toJson() => {
    'claktory': claktory,
    'kind': kind.name,
    'target': target,
    'label': label,
    'made_ms': made.millisecondsSinceEpoch,
    'duration_ms': duration.inMilliseconds,
    'file': path.split('/').last,
    'client_id': clientId,
    'upload_id': uploadId,
    'request_id': requestId,
    'uploaded': uploaded,
  };

  /// The recording kept in [folder], from its kept.json; null if there's
  /// none, or it can't be read.
  static KeptVoice? read(Directory folder) {
    try {
      final record = File('${folder.path}/$recordName');
      if (!record.existsSync()) return null;
      final json = jsonDecode(record.readAsStringSync());
      if (json is! Map<String, Object?>) return null;
      final kind = KeptKind.values
          .where((kind) => kind.name == json['kind'])
          .firstOrNull;
      final file = json['file'];
      if (kind == null || file is! String || file.contains('/')) return null;
      final path = '${folder.path}/$file';
      if (!File(path).existsSync()) return null;
      int number(String key) => (json[key] as num?)?.toInt() ?? 0;
      String text(String key) => json[key] as String? ?? '';
      return KeptVoice(
        path: path,
        duration: Duration(milliseconds: number('duration_ms')),
        claktory: text('claktory'),
        kind: kind,
        target: text('target'),
        label: text('label'),
        made: DateTime.fromMillisecondsSinceEpoch(number('made_ms')),
        clientId: text('client_id'),
        uploadId: text('upload_id').isEmpty ? null : text('upload_id'),
        requestId: text('request_id').isEmpty ? null : text('request_id'),
        uploaded: json['uploaded'] == true,
      );
    } on Object catch (error) {
      developer.log('could not read a kept recording: $error', name: 'voice');
      return null;
    }
  }
}

/// Where recordings are made and kept until sent: the device's private
/// folder for them, or null where it has none (then they go to the
/// temporary folder and live only as long as the app). Tests set another.
Future<String?> Function() keptVoiceFolder = _deviceFolder;

Future<String?>? _deviceFolderAsked;

Future<String?> _deviceFolder() => _deviceFolderAsked ??= _askDevice();

Future<String?> _askDevice() async {
  if (Platform.isAndroid) {
    // The app's own no_backup folder (Context.getNoBackupFilesDir), beside
    // its cache, the temporary folder: private to the app, and the app is
    // never backed up anyway (allowBackup is off).
    final folder = Directory(
      '${Directory.systemTemp.parent.path}/no_backup/voice-not-sent',
    );
    try {
      await folder.create(recursive: true);
      return folder.path;
    } on FileSystemException {
      return null;
    }
  }
  try {
    return await const MethodChannel('factory/recorder')
        .invokeMethod<String>('voiceFolder');
  } on MissingPluginException {
    return null;
  } on Object catch (error) {
    // Refused natively, or no platform at all (a test without one).
    developer.log('no folder for recordings: $error', name: 'voice');
    return null;
  }
}

/// Where a new recording goes, made [now]: a folder of its own in the
/// kept folder, or in the temporary folder where there's none.
Future<({String path, bool kept})> newRecordingPath(DateTime now) async {
  final folder = await keptVoiceFolder();
  if (folder == null) return (path: voiceFilePath(now), kept: false);
  final name = voiceFilePath(now).split('/').last;
  final id = '${now.microsecondsSinceEpoch}-${newVoiceId().substring(0, 6)}';
  return (path: '$folder/$id/$name', kept: true);
}

/// A recording that never got its kept.json is older than this when
/// it's swept: one cut off by the app's end (it can't be read without its
/// end), or cancelled.
const _strayAfter = Duration(hours: 1);

/// [target]'s Claktory in the kept folder, by hash: 'local', its endpoint
/// id, or for a code from before the tunnel its address with its
/// certificate's fingerprint, so another Claktory at the same address
/// later (an address handed to another machine) never gets its recordings.
String keptClaktoryKey(RemoteFactory? target) {
  final name = target == null
      ? 'local'
      : target.endpointId.isNotEmpty
      ? target.endpointId
      : '${target.host}_${target.port}_${target.fingerprint}';
  return crypto.sha256.convert(utf8.encode(name)).toString();
}

/// Deletes every recording kept for [key]'s Claktory: it was forgotten on
/// this device, so they can never be sent. Never throws.
Future<void> forgetKeptVoices(String key) async {
  try {
    final path = await keptVoiceFolder();
    if (path == null) return;
    final root = Directory(path);
    if (!root.existsSync()) return;
    for (final entry in root.listSync().whereType<Directory>()) {
      if (KeptVoice.read(entry)?.claktory == key) {
        await entry.delete(recursive: true);
      }
    }
  } on Object catch (error) {
    developer.log('could not forget recordings: $error', name: 'voice');
  }
}

/// The recordings kept for one connection's Claktory: loaded once it
/// connects, voice messages handed back to the outboxes and dictations
/// kept for their fields; the ones no field has taken listed for the
/// person to play or delete (Settings → Voice).
class KeptVoices extends ChangeNotifier {
  KeptVoices._(this.backend) {
    backend.connected.addListener(_connected);
    _connected();
  }

  static final _of = Expando<KeptVoices>('kept voices');

  /// [backend]'s; made the first time it's asked for.
  static KeptVoices of(Backend backend) =>
      _of[backend] ??= KeptVoices._(backend);

  final Backend backend;

  /// The Claktory the connection shows now.
  String get claktory => keptClaktoryKey(backend.target);

  final _dictations = <KeptVoice>[];
  final _taken = <KeptVoice>{};
  final _loaded = <String>{};

  /// Dictations kept for this Claktory that no field has now: its card
  /// answered elsewhere, or not opened since.
  List<KeptVoice> get loose => [
    for (final voice in _dictations)
      if (!_taken.contains(voice) && voice.claktory == claktory) voice,
  ];

  void _connected() {
    if (!backend.connected.value) return;
    final key = claktory;
    if (!_loaded.add(key)) return;
    unawaited(load(key));
  }

  /// Reads what's kept for [key]'s Claktory: voice messages back to their
  /// outboxes (to send again), dictations here, for their fields. What's
  /// left of recordings that were never kept (the app ended while one was
  /// made) is swept.
  @visibleForTesting
  Future<void> load(String key) async {
    final path = await keptVoiceFolder();
    if (path == null) return;
    final root = Directory(path);
    if (!root.existsSync()) return;
    final found = <KeptVoice>[];
    for (final entry in root.listSync().whereType<Directory>()) {
      final voice = KeptVoice.read(entry);
      if (voice == null) {
        try {
          final age = DateTime.now().difference(entry.statSync().modified);
          if (age > _strayAfter) entry.deleteSync(recursive: true);
        } on FileSystemException {
          // Gone already.
        }
        continue;
      }
      if (voice.claktory == key) found.add(voice);
    }
    found.sort((a, b) => a.made.compareTo(b.made));
    for (final voice in found) {
      switch (voice.kind) {
        case KeptKind.agent:
          backend.outbox.restore(voice);
        case KeptKind.chat:
          backend.chatOutbox.restore(voice);
        case KeptKind.dictation:
          if (!_dictations.any((kept) => kept.path == voice.path)) {
            _dictations.add(voice);
          }
      }
    }
    notifyListeners();
  }

  /// Whether a dictation for the field [target] is kept, no field having
  /// it now.
  bool holds(String target) =>
      target.isNotEmpty && loose.any((voice) => voice.target == target);

  /// A field's new dictation, taken by it: kept on disk.
  Future<void> add(KeptVoice voice) async {
    _dictations.add(voice);
    _taken.add(voice);
    await voice.save();
    notifyListeners();
  }

  /// The dictation kept for the field [target], for it to take, if there
  /// is one no other field has.
  KeptVoice? take(String target) {
    if (target.isEmpty) return null;
    for (final voice in loose) {
      if (voice.target == target) {
        _taken.add(voice);
        _later();
        return voice;
      }
    }
    return null;
  }

  /// A field let go of [voice], not sent: it's listed until a field takes
  /// it again, or the person deletes it.
  void release(KeptVoice voice) {
    if (_taken.remove(voice)) _later();
  }

  var _telling = false;

  /// Tells listeners once what's building or going now is done: fields
  /// take and let go of recordings as they're built and disposed.
  void _later() {
    if (_telling) return;
    _telling = true;
    scheduleMicrotask(() {
      _telling = false;
      if (!_disposed) notifyListeners();
    });
  }

  var _disposed = false;

  /// [voice] is done with: its words came, or the person deleted it.
  Future<void> forget(KeptVoice voice) async {
    _dictations.remove(voice);
    _taken.remove(voice);
    notifyListeners();
    await voice.delete();
  }

  @override
  void dispose() {
    _disposed = true;
    backend.connected.removeListener(_connected);
    super.dispose();
  }
}
