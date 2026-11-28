// Recordings kept on this device until their Claktory has them (an
// unsent recording stays on the device until the Claktory confirms it
// got it, then it's deleted). A voice message to an agent or a chat, or
// a field's dictation, is recorded straight into a private folder
// (Recorder.swift's voiceFolder: never backed up, this user's only;
// Android's no_backup folder), with what it's for beside it (kept.json),
// so a dropped connection, the app quit or killed, loses nothing: on the
// next launch it comes back where it belongs and is sent again, with the
// same ids, so nothing doubles.
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

/// A file going with a kept voice message: where it is on this
/// device and its upload's id, so the message goes again whole.
class KeptFile {
  const KeptFile({
    required this.path,
    required this.name,
    required this.size,
    required this.uploadId,
  });

  final String path;
  final String name;
  final int size;
  final String uploadId;

  /// As kept beside its voice message in [folder]: by its name there when
  /// it's kept with it ([keepWith]), so it's found again wherever the
  /// system puts the app's folders after a relaunch or an update;
  /// by its path when it stayed where the person chose it.
  Map<String, Object?> toJson({String? folder}) => {
    if (folder != null && path.startsWith('$folder/'))
      'file': path.substring(folder.length + 1)
    else
      'path': path,
    'name': name,
    'size': size,
    'upload_id': uploadId,
  };

  /// Read back from beside its voice message in [folder].
  static KeptFile? fromJson(Object? json, {String? folder}) {
    if (json is! Map<String, Object?>) return null;
    final inFolder = json['file'];
    final name = json['name'];
    final upload = json['upload_id'];
    final String path;
    if (inFolder is String && folder != null && !inFolder.contains('/')) {
      path = '$folder/$inFolder';
    } else if (json['path'] case final String kept) {
      path = _found(kept);
    } else {
      return null;
    }
    if (name is! String || upload is! String) return null;
    return KeptFile(
      path: path,
      name: name,
      size: (json['size'] as num?)?.toInt() ?? 0,
      uploadId: upload,
    );
  }

  /// [path], kept by an older app: a copy in the app's temporary folder
  /// found again under that folder as it is now, when the system moved it.
  static String _found(String path) {
    if (File(path).existsSync()) return path;
    const mark = '/$attachmentsFolder/';
    final at = path.indexOf(mark);
    if (at < 0) return path;
    for (final folder in appCopyFolders()) {
      final now = '${folder.path}${path.substring(at)}';
      if (File(now).existsSync()) return now;
    }
    return path;
  }
}

/// Where the iPhone's and Android's pickers put the copies they make
/// (AppDelegate.swift's Attach, AttachmentPicker.kt), in one of
/// [appCopyFolders].
const attachmentsFolder = 'Attachments';

/// The app's temporary folder, where its pickers' copies and pasted images
/// go: the system may clear it while the app isn't running, and Android's
/// picker prunes its copies after a day. Tests set another.
Directory Function() appTempFolder = () => Directory.systemTemp;

/// The folders the app's own copies are in: its temporary folder, and on
/// Android the app's cache folder beside it (Context.getCacheDir), where
/// its picker copies; Dart's temporary folder there is code_cache.
List<Directory> appCopyFolders() => [
  appTempFolder(),
  if (defaultTargetPlatform == TargetPlatform.android)
    Directory('${appTempFolder().parent.path}/cache'),
];

/// Moves [file], one of the app's own temporary copies (a photo picked or
/// pasted), into [folder], a voice message's kept folder, as its [index]th
/// file, so it stays until the Claktory has the message; answers
/// where it is now. A file the person chose where it lives stays there.
Future<String> keepWith(String file, String folder, int index) async {
  final copy = appCopyFolders().any((temp) => file.startsWith('${temp.path}/'));
  if (!copy || file.startsWith('$folder/')) return file;
  final target = '$folder/$index-${file.split('/').last}';
  try {
    await File(file).rename(target);
    return target;
  } on FileSystemException {
    try {
      // Not on one disk: copied, then the copy left behind goes.
      await File(file).copy(target);
      await File(file).delete();
      return target;
    } on FileSystemException {
      return file;
    }
  }
}

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
    this.text = '',
    this.files = const [],
    this.draft = false,
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

  /// A voice message's typed text, sent with it as one message.
  String text;

  /// The other files going with a voice message, after the
  /// recording.
  List<KeptFile> files;

  /// A voice note waiting in its message box, not sent yet: after a
  /// relaunch it goes back to the box, not to the outbox.
  bool draft;

  /// A dictation's words, already being asked for: it was finished from
  /// the floating bar with its field gone, so they're ready when
  /// the field is back. In memory only; the field takes it.
  Future<String>? heard;

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
    if (text.isNotEmpty) 'text': text,
    if (files.isNotEmpty)
      'files': [for (final file in files) file.toJson(folder: folder.path)],
    if (draft) 'draft': true,
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
        text: text('text'),
        files: [
          if (json['files'] case final List<Object?> files)
            for (final file in files)
              ?KeptFile.fromJson(file, folder: folder.path),
        ],
        draft: json['draft'] == true,
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

/// A recording's folder with nothing in it is older than this when it's
/// swept: the microphone gave up the file without the app knowing (the
/// app closed while recording, as Android's Back can do), never a
/// recording starting now, whose file comes with its folder.
const _emptyAfter = Duration(minutes: 1);

/// Sweeps [entry], a folder in the kept one that holds no recording kept
/// for anything: once empty for a minute, or left over for an hour.
void _sweepStray(Directory entry) {
  try {
    final age = DateTime.now().difference(entry.statSync().modified);
    if (age > _strayAfter || (age > _emptyAfter && entry.listSync().isEmpty)) {
      entry.deleteSync(recursive: true);
    }
  } on FileSystemException {
    // Gone already.
  }
}

/// Sweeps the kept folder's empty folders, left by recordings the app
/// never heard the end of: when a recording starts, so
/// they don't wait for the next launch.
Future<void> sweepEmptyRecordings() async {
  final path = await keptVoiceFolder();
  if (path == null) return;
  try {
    for (final entry in Directory(path).listSync().whereType<Directory>()) {
      try {
        if (entry.listSync().isEmpty) _sweepStray(entry);
      } on FileSystemException {
        // Gone already.
      }
    }
  } on FileSystemException {
    // No kept folder yet.
  }
}

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

/// How many recordings are kept on this device for the Claktory [key]
/// ([keptClaktoryKey]), not sent yet: what forgetting it deletes too.
Future<int> keptVoiceCount(String key) async {
  try {
    final path = await keptVoiceFolder();
    if (path == null) return 0;
    final root = Directory(path);
    if (!root.existsSync()) return 0;
    var count = 0;
    for (final entry in root.listSync().whereType<Directory>()) {
      if (KeptVoice.read(entry)?.claktory == key) count++;
    }
    return count;
  } on Object catch (error) {
    developer.log('could not count recordings: $error', name: 'voice');
    return 0;
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
        _sweepStray(entry);
        continue;
      }
      if (voice.claktory == key) found.add(voice);
    }
    found.sort((a, b) => a.made.compareTo(b.made));
    for (final voice in found) {
      switch (voice.kind) {
        // A voice note left in its message box: back in the box.
        case KeptKind.agent || KeptKind.chat when voice.draft:
          VoiceNotes.of(backend).restore(voice);
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

/// The voice notes waiting in message boxes, one per box at most,
/// by the box's key ([MessageTarget.key]): recorded while the box had text
/// (or files), they go with it as one message when it's sent. Kept on the
/// device as any recording not sent yet ([KeptVoice.draft]), so a relaunch
/// puts each back in its box; the rest of the box (its text, its files)
/// is the drafts'.
class VoiceNotes extends ChangeNotifier {
  VoiceNotes._();

  static final _of = Expando<VoiceNotes>('voice notes');

  /// [backend]'s; made the first time it's asked for.
  static VoiceNotes of(Backend backend) => _of[backend] ??= VoiceNotes._();

  final _byBox = <String, KeptVoice>{};

  /// The note waiting in the box [box], if any.
  KeptVoice? note(String box) => _byBox[box];

  /// [voice] waits in the box [box] from now on (its target), replacing
  /// what waited there.
  Future<void> put(String box, KeptVoice voice) async {
    final was = _byBox[box];
    voice.draft = true;
    _byBox[box] = voice;
    notifyListeners();
    if (was != null && was != voice) await was.delete();
    await voice.save();
  }

  /// One kept from before a relaunch, back in its box.
  void restore(KeptVoice voice) {
    if (voice.target.isEmpty || _byBox[voice.target] != null) return;
    _byBox[voice.target] = voice;
    notifyListeners();
  }

  /// Takes the note out of the box [box] to send it: still on the device,
  /// for the outbox to keep until the Claktory has it.
  KeptVoice? take(String box) {
    final voice = _byBox.remove(box);
    if (voice == null) return null;
    voice.draft = false;
    notifyListeners();
    return voice;
  }

  /// The person removed the note from the box [box]: gone from the device.
  Future<void> discard(String box) async {
    final voice = _byBox.remove(box);
    if (voice == null) return;
    notifyListeners();
    await voice.delete();
  }
}
