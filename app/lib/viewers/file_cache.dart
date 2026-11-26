// What a device keeps of the files its viewers read from a Claktory on
// another machine (#446, Matt's decision: device-local state). Coming back
// to a picture, a video, a PDF or a text an agent showed shows the copy
// kept here at once, once a one-byte read says the file is still that
// version, rather than bringing it over the tunnel again; a file changed
// under the same name is read again and replaces it. Offline, the copy
// shows, saying it may be out of date.
//
// - Kept per Claktory ([keptClaktoryKey]), path and variant (the original,
//   or the preview made for this screen), each with the version it is
//   (FileContent.sha256, the original's for a preview too).
// - In the device's caches folder, never backed up: Library/Caches on an
//   iPhone, iPad or Mac, the app's cache on Android.
// - Each Claktory's copies keep to what the person set for this device on
//   it (Settings.file_cache, 1 GB until changed): past it, the copies used
//   longest ago go. Settings shows how much is kept, with Clear.
// - Forgetting a Claktory on this device deletes its copies.
// - This machine's own Claktory reads its files directly: nothing is kept.
import 'dart:async';
import 'dart:convert';
import 'dart:developer' as developer;
import 'dart:io';

import 'package:crypto/crypto.dart' as crypto;
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart' show MethodChannel;

import '../backend.dart';
import '../paced.dart' show PreviewBox;
import '../protocol/factory/v1/factory.pb.dart';
import '../voice_keep.dart' show keptClaktoryKey;

/// What a device keeps of one Claktory's files until the person changes it.
const defaultFileCacheBytes = 1000000000;

/// The sizes Settings offers; the backend takes 100 MB to 100 GB.
const fileCacheChoices = [
  100000000,
  250000000,
  500000000,
  1000000000,
  2000000000,
  5000000000,
  10000000000,
  20000000000,
];

/// The variant of a file that is the file itself.
const originalVariant = 'original';

/// The variant that is a preview made for [box] (#379).
String previewVariant(PreviewBox box) => 'preview-${box.width}x${box.height}';

/// How much of [backend]'s Claktory's files this device keeps, as set on
/// it for this device.
int fileCacheLimit(Backend backend) {
  final bytes = backend.workspace.workspace.settings.fileCache[backend.zoomKey];
  if (bytes == null || bytes <= 0) return defaultFileCacheBytes;
  return bytes.toInt();
}

/// Whether this device keeps [backend]'s files: a Claktory on another
/// machine. This Mac's own reads them directly.
@visibleForTesting
bool Function(Backend backend) keepsFilesOf = (backend) =>
    backend.target != null;

/// Whether this device keeps [backend]'s files (Settings → Storage shows
/// for it).
bool keepsFiles(Backend backend) => keepsFilesOf(backend);

/// The folder the copies are kept in, one folder in it per Claktory; null
/// where there's none.
@visibleForTesting
Future<String?> Function() fileCacheFolder = _deviceFolder;

Future<String?>? _deviceFolderAsked;

Future<String?> _deviceFolder() => _deviceFolderAsked ??= _findFolder();

Future<String?> _findFolder() async {
  final home = Platform.environment['HOME'] ?? '';
  final String path;
  if (Platform.isIOS) {
    // The app's container's Library/Caches, beside its tmp: never backed
    // up.
    path = '${Directory.systemTemp.parent.path}/Library/Caches/files';
    // Unreadable while the iPhone is locked (Fugo 33088): nothing is kept
    // where that can't be made so.
    if (!await _protect(path)) return null;
  } else if (Platform.isAndroid) {
    // The app's cache folder (Context.getCacheDir), beside the temporary
    // folder, which is code_cache: Android empties that at every update
    // (Sale's row 128).
    path = '${Directory.systemTemp.parent.path}/cache/files';
  } else if (Platform.isMacOS) {
    const app = bool.fromEnvironment('FACTORY_DEV')
        ? 'dev.factory.app.dev'
        : 'dev.factory.app';
    path = '$home/Library/Caches/$app/files';
  } else if (Platform.isWindows) {
    // Cache in the app's own folder in %LOCALAPPDATA%, this account's only
    // (app/windows/runner/device_channel.cpp); Windows has no HOME.
    try {
      final cache = await const MethodChannel('factory/device')
          .invokeMethod<String>('cacheFolder');
      if (cache == null || cache.isEmpty) return null;
      path = '$cache\\files';
    } on Object catch (error) {
      developer.log('no folder for kept files: $error', name: 'files');
      return null;
    }
  } else if (home.isNotEmpty) {
    const app = bool.fromEnvironment('FACTORY_DEV')
        ? 'claktory-dev'
        : 'claktory';
    path = '$home/.cache/$app/files';
  } else {
    return null;
  }
  // Made when the first copy is kept.
  return path;
}

/// iOS: [path], a folder (made if it isn't there) and what's in it, or a
/// file moved into one, can't be opened while the iPhone is locked
/// (Fugo 33088, 33120): whether it could be made so.
Future<bool> _protect(String path) async {
  try {
    await const MethodChannel('factory/device')
        .invokeMethod<void>('protectKeptFiles', path);
    return true;
  } on Object catch (error) {
    developer.log('could not protect kept files: $error', name: 'files');
    return false;
  }
}

/// Protects [file], about to be moved in from elsewhere, where the device
/// can: moved, it keeps the class it was made with, unlike a file made in
/// the folder.
@visibleForTesting
Future<bool> Function(File file) protectMovedIn = (file) async =>
    !Platform.isIOS || await _protect(file.path);

/// One file's copy kept on this device.
class KeptFile {
  KeptFile._({
    required this.id,
    required this.path,
    required this.variant,
    required this.content,
    required this.file,
    required this.bytes,
    required this.used,
  });

  /// Its path's and variant's hash: the name its record is kept under.
  final String id;
  final String path;
  final String variant;

  /// What its read said, without the bytes: its version, its type, a
  /// preview's sizes.
  final FileContent content;

  /// Its bytes.
  final File file;
  final int bytes;

  /// When it was last used, in ms since 1970.
  int used;

  String get sha256 => content.sha256;

  /// It, as a read of it would answer.
  Future<FileContent> read() async =>
      content.deepCopy()..data = await file.readAsBytes();

  Map<String, Object?> _toJson() => {
    'path': path,
    'variant': variant,
    'file': file.uri.pathSegments.last,
    'bytes': bytes,
    'used': used,
    'content': base64.encode(content.writeToBuffer()),
  };

  static KeptFile? _fromJson(String id, Directory folder, Object? json) {
    if (json is! Map<String, Object?>) return null;
    final path = json['path'];
    final variant = json['variant'];
    final file = json['file'];
    final content = json['content'];
    if (path is! String ||
        variant is! String ||
        file is! String ||
        file.contains('/') ||
        content is! String) {
      return null;
    }
    try {
      return KeptFile._(
        id: id,
        path: path,
        variant: variant,
        content: FileContent.fromBuffer(base64.decode(content)),
        file: File('${folder.path}/$file'),
        bytes: (json['bytes'] as num?)?.toInt() ?? 0,
        used: (json['used'] as num?)?.toInt() ?? 0,
      );
    } on FormatException {
      return null;
    }
  }
}

/// A kept copy found for a file, and whether the file is still its
/// version: [current] false when that couldn't be asked (offline), so it
/// may be out of date.
typedef KeptRead = ({KeptFile kept, bool current});

/// One Claktory's files kept on this device.
class FileCache {
  FileCache._(this.key);

  static final _caches = <String, FileCache>{};

  /// [backend]'s Claktory's, held to what's set for this device on it;
  /// null where nothing is kept (this machine's own Claktory).
  static FileCache? of(Backend backend) {
    if (!keepsFilesOf(backend)) return null;
    final key = keptClaktoryKey(backend.target);
    final cache = _caches[key] ??= FileCache._(key);
    final limit = fileCacheLimit(backend);
    if (limit != cache.limit) {
      cache.limit = limit;
      // Made smaller: what's past it goes now.
      if (cache._entries != null) unawaited(cache._serial(cache._evict));
    }
    return cache;
  }

  /// Deletes every copy kept for the Claktory [key] ([keptClaktoryKey]):
  /// it was forgotten on this device. Never throws.
  static Future<void> forget(String key) async {
    final cache = _caches.remove(key);
    try {
      if (cache != null) {
        await cache._serial(() async {
          cache._entries = {};
          cache._changed();
        });
      }
      final root = await fileCacheFolder();
      if (root == null) return;
      final folder = Directory('$root/$key');
      if (await folder.exists()) await folder.delete(recursive: true);
    } on Object catch (error) {
      developer.log('could not forget kept files: $error', name: 'files');
    }
  }

  @visibleForTesting
  static void forgetAllInMemory() => _caches.clear();

  /// The Claktory's [keptClaktoryKey].
  final String key;

  /// Past this many bytes, the copies used longest ago go.
  int limit = defaultFileCacheBytes;

  /// How many bytes are kept: null until they've been counted.
  final used = ValueNotifier<int?>(null);

  /// By id; null until read from the folder.
  Map<String, KeptFile>? _entries;
  Directory? _folder;

  /// Files a viewer has open (a video playing, a PDF shown), by path, with
  /// how many hold each: never deleted until let go.
  final _held = <String, int>{};

  /// Held files no longer kept: deleted once let go.
  final _doomed = <String>{};

  /// One change to the folder at a time.
  Future<void> _last = Future.value();

  Future<T> _serial<T>(Future<T> Function() work) {
    final done = _last.then((_) => work());
    _last = done.then((_) {}, onError: (_) {});
    return done;
  }

  void _changed() {
    final entries = _entries;
    used.value = entries?.values.fold<int>(0, (sum, kept) => sum + kept.bytes);
  }

  static String _idOf(String path, String variant) => crypto.sha256
      .convert(utf8.encode('$path\n$variant'))
      .toString()
      .substring(0, 32);

  /// Reads what's kept from the folder, once; sweeps what no record names.
  Future<Map<String, KeptFile>> _load() async {
    final loaded = _entries;
    if (loaded != null) return loaded;
    final entries = <String, KeptFile>{};
    final root = await fileCacheFolder();
    if (root != null) {
      final folder = Directory('$root/$key');
      try {
        await folder.create(recursive: true);
        _folder = folder;
        final records = <File>[];
        final others = <File>[];
        await for (final entry in folder.list()) {
          if (entry is! File) continue;
          (entry.path.endsWith('.json') ? records : others).add(entry);
        }
        for (final record in records) {
          final id = record.uri.pathSegments.last.replaceAll('.json', '');
          KeptFile? kept;
          try {
            kept = KeptFile._fromJson(
              id,
              folder,
              jsonDecode(await record.readAsString()),
            );
          } on FormatException {
            kept = null;
          }
          if (kept == null || !await kept.file.exists()) {
            await _delete(record);
            continue;
          }
          entries[id] = kept;
        }
        final named = {for (final kept in entries.values) kept.file.path};
        for (final other in others) {
          if (!named.contains(other.path) && !_held.containsKey(other.path)) {
            await _delete(other);
          }
        }
      } on FileSystemException catch (error) {
        developer.log('could not read kept files: $error', name: 'files');
      }
    }
    _entries = entries;
    _changed();
    return entries;
  }

  /// How many bytes are kept, counted now.
  Future<int> usage() async {
    await _serial(_load);
    return used.value ?? 0;
  }

  /// The copy kept of [path]'s [variant], whichever version; null if none.
  Future<KeptFile?> find(String path, String variant) => _serial(() async {
    final entries = await _load();
    return entries[_idOf(path, variant)];
  });

  /// The first of [variants] of [path] kept here, if the file is still its
  /// version: asked with a one-byte read. A file changed or gone has its
  /// copies dropped (null); one that can't be asked (offline) gives its
  /// copy all the same, not [KeptRead.current].
  Future<KeptRead?> lookup(
    Backend backend,
    String path,
    List<String> variants,
  ) async {
    KeptFile? kept;
    for (final variant in variants) {
      kept = await find(path, variant);
      if (kept != null) break;
    }
    if (kept == null || kept.sha256.isEmpty) return null;
    var current = false;
    if (backend.connected.value) {
      try {
        final now = await backend.readFile(
          path,
          maxBytes: 1,
          expectedSha256: kept.sha256,
        );
        if (now.error.isNotEmpty) {
          // Changed, or gone: what comes next says which.
          await drop(path);
          return null;
        }
        current = true;
      } on RequestFailure {
        current = false;
      }
    }
    await touch(kept);
    return (kept: kept, current: current);
  }

  /// [kept] was used now: the last to go.
  Future<void> touch(KeptFile kept) => _serial(() async {
    kept.used = DateTime.now().millisecondsSinceEpoch;
    if (_entries?[kept.id] == kept) await _writeRecord(kept);
  });

  /// Keeps [content], a whole read of [path] (its bytes in it), as its
  /// [variant]. Nothing is kept for a read without a version.
  Future<KeptFile?> keepRead(
    String path,
    String variant,
    FileContent content,
  ) => _serial(() async {
    if (content.sha256.isEmpty || content.error.isNotEmpty) return null;
    if (content.data.length > limit) return null;
    await _load();
    final folder = _folder;
    if (folder == null) return null;
    final file = _newFile(folder, path, variant);
    final part = File('${file.path}.part');
    try {
      await part.writeAsBytes(content.data, flush: true);
      await part.rename(file.path);
    } on FileSystemException catch (error) {
      developer.log('could not keep $path: $error', name: 'files');
      await _delete(part);
      return null;
    }
    final meta = content.deepCopy()..data = const [];
    return _put(path, variant, meta, file, content.data.length);
  });

  /// Keeps [from], a whole copy of [path] written elsewhere on this
  /// device (a video or a PDF read a range at a time), as its [variant]:
  /// moved in, and its place here answered. Null, and [from] left as it
  /// is, when it's more than this Claktory keeps, or can't be moved.
  Future<KeptFile?> adopt(
    String path,
    String variant,
    FileContent content,
    File from,
  ) => _serial(() async {
    if (content.sha256.isEmpty) return null;
    await _load();
    final folder = _folder;
    if (folder == null) return null;
    final int bytes;
    try {
      bytes = await from.length();
    } on FileSystemException {
      return null;
    }
    if (bytes > limit) return null;
    // Moved in, it keeps its class: protected first, where it is.
    if (!await protectMovedIn(from)) return null;
    final file = _newFile(folder, path, variant);
    try {
      await from.rename(file.path);
    } on FileSystemException {
      // On another volume: copied, then the first deleted.
      try {
        await from.copy(file.path);
        await _delete(from);
      } on FileSystemException catch (error) {
        developer.log('could not keep $path: $error', name: 'files');
        await _delete(file);
        return null;
      }
    }
    final meta = content.deepCopy()..data = const [];
    return _put(path, variant, meta, file, bytes);
  });

  /// A new name for a copy, never one in use: a copy replaced while it
  /// plays stays where the player has it.
  File _newFile(Directory folder, String path, String variant) {
    final name = path.split('/').last;
    final dot = name.lastIndexOf('.');
    final ext = dot > 0 ? name.substring(dot) : '';
    final safe = RegExp(r'^\.[A-Za-z0-9]{1,8}$').hasMatch(ext) ? ext : '';
    final id = _idOf(path, variant);
    return File(
      '${folder.path}/$id-${DateTime.now().microsecondsSinceEpoch}$safe',
    );
  }

  Future<KeptFile?> _put(
    String path,
    String variant,
    FileContent meta,
    File file,
    int bytes,
  ) async {
    final entries = await _load();
    final id = _idOf(path, variant);
    final old = entries.remove(id);
    if (old != null) await _deleteCopy(old);
    final kept = KeptFile._(
      id: id,
      path: path,
      variant: variant,
      content: meta,
      file: file,
      bytes: bytes,
      used: DateTime.now().millisecondsSinceEpoch,
    );
    entries[id] = kept;
    await _writeRecord(kept);
    await _evict(keep: kept);
    _changed();
    return entries[id];
  }

  Future<void> _writeRecord(KeptFile kept) async {
    final folder = _folder;
    if (folder == null) return;
    final record = File('${folder.path}/${kept.id}.json');
    final part = File('${record.path}.part');
    try {
      await part.writeAsString(jsonEncode(kept._toJson()), flush: true);
      await part.rename(record.path);
    } on FileSystemException catch (error) {
      developer.log('could not keep a record: $error', name: 'files');
    }
  }

  /// Past [limit]: the copies used longest ago go, but never one held
  /// open, nor [keep].
  Future<void> _evict({KeptFile? keep}) async {
    final entries = _entries;
    if (entries == null) return;
    var total = entries.values.fold<int>(0, (sum, kept) => sum + kept.bytes);
    if (total <= limit) return;
    final oldest = entries.values.toList()
      ..sort((a, b) => a.used.compareTo(b.used));
    for (final kept in oldest) {
      if (total <= limit) break;
      if (kept == keep || _held.containsKey(kept.file.path)) continue;
      entries.remove(kept.id);
      await _deleteCopy(kept);
      total -= kept.bytes;
    }
    _changed();
  }

  /// Drops every copy of [path]: it changed, or it's gone.
  Future<void> drop(String path) => _serial(() async {
    final entries = await _load();
    for (final kept in entries.values.toList()) {
      if (kept.path != path) continue;
      entries.remove(kept.id);
      await _deleteCopy(kept);
    }
    _changed();
  });

  /// Deletes every copy kept for this Claktory (Settings → Clear); one a
  /// viewer has open goes once it's let go.
  Future<void> clear() => _serial(() async {
    final entries = await _load();
    for (final kept in entries.values.toList()) {
      await _deleteCopy(kept);
    }
    entries.clear();
    _changed();
  });

  Future<void> _deleteCopy(KeptFile kept) async {
    final folder = _folder;
    if (folder != null) await _delete(File('${folder.path}/${kept.id}.json'));
    if (_held.containsKey(kept.file.path)) {
      _doomed.add(kept.file.path);
    } else {
      await _delete(kept.file);
    }
  }

  /// A viewer has [file] open: it stays until [release]d.
  void hold(File file) => _held[file.path] = (_held[file.path] ?? 0) + 1;

  /// Let go of [file]; deleted now if it's no longer kept.
  void release(File file) {
    final count = (_held[file.path] ?? 1) - 1;
    if (count > 0) {
      _held[file.path] = count;
      return;
    }
    _held.remove(file.path);
    if (_doomed.remove(file.path)) unawaited(_delete(file));
  }

  static Future<void> _delete(File file) async {
    try {
      await file.delete();
    } on FileSystemException {
      // Gone already.
    }
  }
}
