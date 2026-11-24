import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;

import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';

import 'paced.dart' show Pace;

/// A piece of a release's app archive, as the backend's FetchApp answers:
/// the gzipped tar of the release's `.app`, with what the whole archive is.
class AppPiece {
  const AppPiece({
    this.preparing = false,
    this.size = 0,
    this.sha256 = '',
    this.appName = '',
    this.data = const [],
  });

  /// The backend is still packing the archive; ask again shortly.
  final bool preparing;

  /// The whole archive's size in bytes, and its SHA-256 in hex.
  final int size;
  final String sha256;

  /// The bundle at the archive's top level, e.g. "Factory.app".
  final String appName;

  /// The bytes from the offset asked for.
  final List<int> data;
}

/// Asks the backend for up to [maxBytes] of the archive of release
/// [version] from [offset] (0: as much as it sends at once, 1 MiB).
typedef FetchAppPiece = Future<AppPiece> Function(
  String version,
  int offset, {
  int maxBytes,
});

/// Why an update of this app from a Factory on another machine stopped.
class UpdateFailure implements Exception {
  const UpdateFailure(this.message);

  final String message;

  @override
  String toString() => message;
}

/// Where an update of this app is.
enum UpdateStage {
  /// The other machine is packing its app.
  preparing,
  downloading,

  /// The connection dropped: the download carries on once it's back.
  waiting,

  /// Checking the download and the bundle in it.
  checking,
}

class UpdateProgress {
  const UpdateProgress(this.stage, {this.received = 0, this.size = 0});

  final UpdateStage stage;

  /// Bytes downloaded so far, of [size].
  final int received;
  final int size;

  double get fraction => size <= 0 ? 0 : (received / size).clamp(0.0, 1.0);
}

/// A downloaded and checked bundle, ready to replace this app's.
class StagedApp {
  const StagedApp({required this.app, required this.staging});

  /// The new bundle, inside [staging].
  final String app;

  /// The folder it was downloaded into, beside this app's bundle, on its
  /// volume; removed once the bundle is in place.
  final String staging;
}

/// What updating asks of macOS: unpacking, reading and checking bundles, and
/// replacing this one once the app has quit.
class AppSystem {
  const AppSystem();

  /// Unpacks [member] (the app; a Linux Factory serves the whole Mac
  /// release, with more beside it) from [archive] into [into].
  Future<void> unpack(String archive, String member, String into) => _run(
    '/usr/bin/tar',
    ['-xzf', archive, '-C', into, member],
    "couldn't unpack the app",
  );

  /// The bundle's CFBundleIdentifier.
  Future<String> bundleId(String app) async => (await _run(
    '/usr/libexec/PlistBuddy',
    ['-c', 'Print :CFBundleIdentifier', '$app/Contents/Info.plist'],
    "couldn't read ${_name(app)}'s bundle identifier",
  )).trim();

  /// Throws unless the bundle's signature holds for all of it.
  Future<void> verifySignature(String app) => _run('/usr/bin/codesign', [
    '--verify',
    '--deep',
    '--strict',
    app,
  ], "the new app's signature isn't valid");

  /// Starts replacing [app] with [staged.app], on its own, once the process
  /// [pid] has quit (stopping it after 10s), then opens it; Factory Dev in
  /// the background, as always. [opener] opens it (tests pass `true`).
  Future<void> startSwap(
    StagedApp staged, {
    required String app,
    required int pid,
    String opener = '/usr/bin/open',
  }) async {
    try {
      await Process.start('/bin/sh', [
        '-c',
        swapScript,
        'sh',
        '$pid',
        app,
        staged.app,
        staged.staging,
        opener,
      ], mode: ProcessStartMode.detached);
    } on ProcessException catch (error) {
      throw UpdateFailure("couldn't start replacing the app: ${error.message}");
    }
  }

  /// Waits for the app to quit, puts the new bundle in place with ditto
  /// (the old one stays if that fails), removes the staging folder and opens
  /// the app.
  static const swapScript =
      r'''pid="$1"; app="$2"; new="$3"; staging="$4"; opener="$5"
for _ in $(seq 100); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
kill "$pid" 2>/dev/null
rm -rf "$app.updating" && ditto "$new" "$app.updating" && rm -rf "$app" && mv "$app.updating" "$app"
rm -rf "$staging" "$app.updating"
case "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")" in
  *.dev) "$opener" -g "$app" ;;
  *) "$opener" "$app" ;;
esac
''';

  static Future<String> _run(
    String command,
    List<String> arguments,
    String failure,
  ) async {
    final ProcessResult result;
    try {
      result = await Process.run(command, arguments);
    } on ProcessException catch (error) {
      throw UpdateFailure('$failure: ${error.message}');
    }
    if (result.exitCode != 0) {
      final said = '${result.stderr}'.trim();
      throw UpdateFailure(said.isEmpty ? failure : '$failure: $said');
    }
    return '${result.stdout}';
  }
}

String _name(String path) => path.substring(path.lastIndexOf('/') + 1);

/// The most one piece of the archive can be: the backend's own size
/// (app_archive.rs's CHUNK), which a backend from before
/// FetchApp.max_bytes sends whatever is asked.
const _mostPerPiece = 1024 * 1024;

/// What a download of an archive kept so far, beside it in the staging
/// folder: which archive it is and how far it has got, so it carries on
/// from there after a failure, a reconnect or a restart of the app.
class _Kept {
  const _Kept({
    required this.version,
    required this.size,
    required this.sha256,
    required this.appName,
    required this.through,
  });

  final String version;
  final int size;
  final String sha256;
  final String appName;

  /// The archive's bytes up to here are in the part file.
  final int through;

  static _Kept? read(File note, String version) {
    try {
      final json = jsonDecode(note.readAsStringSync());
      if (json is! Map) return null;
      final kept = _Kept(
        version: json['version'] as String,
        size: json['size'] as int,
        sha256: json['sha256'] as String,
        appName: json['appName'] as String,
        through: json['through'] as int,
      );
      return kept.version == version ? kept : null;
    } on Object {
      return null;
    }
  }

  void write(File note) => note.writeAsStringSync(
    jsonEncode({
      'version': version,
      'size': size,
      'sha256': sha256,
      'appName': appName,
      'through': through,
    }),
  );
}

/// The archive changed on the other machine, or came out damaged: what was
/// downloaded is thrown away, and the download starts over.
class _StartOver implements Exception {
  const _StartOver(this.message);

  final String message;
}

/// Updates this app from a Factory on another machine: it downloads the
/// release's app over the connection, checks it, and swaps it in once the
/// app has quit. (This Mac's Factory swaps its app itself.)
class AppUpdater {
  const AppUpdater({
    this.system = const AppSystem(),
    this.inFlight = 2,
    this.pollEvery = const Duration(seconds: 1),
    this.patience = const Duration(minutes: 10),
    this.retryEvery = const Duration(seconds: 2),
    this.reconnectWithin = const Duration(hours: 1),
    this.firstPiece = 64 * 1024,
    this.leastPiece = 32 * 1024,
    this.mostPiece = _mostPerPiece,
  });

  final AppSystem system;

  /// Pieces asked for at once. Few, each sized to the link ([Pace]): what's
  /// on its way holds up the connection's pings, and a link that can't
  /// answer one in time is closed.
  final int inFlight;

  /// How often to ask again while the other machine packs its app, and for
  /// how long at most.
  final Duration pollEvery;
  final Duration patience;

  /// How long to wait before asking again for a piece that failed while
  /// the connection stayed up.
  final Duration retryEvery;

  /// How long a download waits for a lost connection to come back before
  /// it stops; what it has is kept for the next try either way.
  final Duration reconnectWithin;

  /// The first piece's size, and the least and most the link's pace ([Pace])
  /// makes one: the most is all a backend sends at once.
  final int firstPiece;
  final int leastPiece;
  final int mostPiece;

  /// Replaces the bundle [app] with [staged] once this process has quit,
  /// then opens it.
  Future<void> replace(StagedApp staged, {required String app}) =>
      system.startSwap(staged, app: app, pid: pid);

  /// Removes a staged app that won't be put in place.
  void discard(StagedApp staged) {
    try {
      Directory(staged.staging).deleteSync(recursive: true);
    } on FileSystemException {
      // The next try starts by clearing it.
    }
  }

  /// Downloads release [version]'s app into a folder beside [bundlePath]
  /// (so on its volume), checks its size and SHA-256, unpacks it, and checks
  /// that it's this app (its bundle identifier) and that its signature
  /// holds. Throws [UpdateFailure] (or the backend's RequestFailure) saying
  /// why it can't.
  ///
  /// The download carries on from what an earlier one kept there, and keeps
  /// what it got when it stops: the next try goes on from there. While
  /// [connected] is false it waits for it, up to [reconnectWithin], and
  /// carries on. Anything else that goes wrong leaves nothing behind.
  Future<StagedApp> stage({
    required FetchAppPiece fetch,
    required String version,
    required String bundlePath,
    ValueListenable<bool>? connected,
    void Function(UpdateProgress progress)? onProgress,
  }) async {
    if (bundlePath.isEmpty) {
      throw const UpdateFailure("this copy of the app can't replace itself");
    }
    final parent = bundlePath.substring(0, bundlePath.lastIndexOf('/'));
    final staging = Directory('$parent/.${_name(bundlePath)}.update');
    try {
      staging.createSync();
      // Only a download in progress is kept: a bundle unpacked by a try
      // that didn't finish goes.
      for (final entry in staging.listSync()) {
        final name = _name(entry.path);
        if (name != _partName && name != _noteName) {
          entry.deleteSync(recursive: true);
        }
      }
    } on FileSystemException catch (error) {
      throw UpdateFailure(
        "can't save the update next to the app, in $parent: ${error.osError?.message ?? error.message}",
      );
    }
    void clear() {
      try {
        staging.deleteSync(recursive: true);
      } on FileSystemException {
        // Nothing more to do; the next try starts by clearing it.
      }
    }

    final String appName;
    try {
      appName = await _download(
        fetch,
        version,
        staging.path,
        connected,
        onProgress,
      );
    } on _StartOver catch (failure) {
      clear();
      throw UpdateFailure(failure.message);
    } on UpdateFailure catch (failure) {
      // Not this app at all: nothing to carry on from.
      if (failure.message.startsWith('the download holds')) clear();
      rethrow;
    }
    try {
      onProgress?.call(const UpdateProgress(UpdateStage.checking));
      final archive = '${staging.path}/$_partName';
      final unpacked = Directory('${staging.path}/unpacked')..createSync();
      await system.unpack(archive, appName, unpacked.path);
      File(archive).deleteSync();
      File('${staging.path}/$_noteName').deleteSync();
      final app = '${unpacked.path}/$appName';
      if (!Directory(app).existsSync()) {
        throw UpdateFailure('the download is missing $appName');
      }
      final ours = await system.bundleId(bundlePath);
      final theirs = await system.bundleId(app);
      if (theirs != ours) {
        throw UpdateFailure(
          'the download is a different app ($theirs, not $ours)',
        );
      }
      await system.verifySignature(app);
      return StagedApp(app: app, staging: staging.path);
    } catch (_) {
      clear();
      rethrow;
    }
  }

  static const _partName = 'app.tar.gz.part';
  static const _noteName = 'download.json';

  /// Downloads the archive into the staging folder's part file, from where
  /// an earlier download got to, and answers the bundle's name in it. A
  /// damaged download is thrown away and downloaded again, once.
  Future<String> _download(
    FetchAppPiece fetch,
    String version,
    String staging,
    ValueListenable<bool>? connected,
    void Function(UpdateProgress)? onProgress,
  ) async {
    try {
      return await _downloadOnce(
        fetch,
        version,
        staging,
        connected,
        onProgress,
      );
    } on _StartOver {
      _forget(staging);
      return _downloadOnce(fetch, version, staging, connected, onProgress);
    }
  }

  void _forget(String staging) {
    for (final name in [_partName, _noteName]) {
      try {
        File('$staging/$name').deleteSync();
      } on FileSystemException {
        // Not there.
      }
    }
  }

  Future<String> _downloadOnce(
    FetchAppPiece fetch,
    String version,
    String staging,
    ValueListenable<bool>? connected,
    void Function(UpdateProgress)? onProgress,
  ) async {
    final part = File('$staging/$_partName');
    final note = File('$staging/$_noteName');
    var kept = _Kept.read(note, version);
    var have = kept == null || !part.existsSync()
        ? 0
        : math.min(kept.through, part.lengthSync());
    if (have == 0) kept = null;
    final pace = Pace(first: firstPiece, least: leastPiece, most: mostPiece);
    // How much has come, for what the progress says while it waits.
    var received = have;
    var size = kept?.size ?? 0;

    /// One piece, asked again while the connection is down or the backend
    /// can't answer for a moment.
    Future<AppPiece> ask(int offset, int maxBytes) async {
      var failures = 0;
      while (true) {
        final started = DateTime.now();
        try {
          final piece = await fetch(version, offset, maxBytes: maxBytes);
          if (!piece.preparing) {
            pace.took(piece.data.length, DateTime.now().difference(started));
          }
          return piece;
        } on Exception {
          if (connected == null) rethrow;
          pace.failed(maxBytes);
          if (!connected.value) {
            onProgress?.call(
              UpdateProgress(
                UpdateStage.waiting,
                received: received,
                size: size,
              ),
            );
            if (!await _connects(connected, reconnectWithin)) {
              throw const UpdateFailure(
                "the connection dropped and didn't come back. What has "
                'downloaded is kept, so trying again carries on from there',
              );
            }
            // A new connection: its speed is learned again.
            pace.relink();
            failures = 0;
            continue;
          }
          if (++failures >= 3) rethrow;
          await Future<void>.delayed(retryEvery);
        }
      }
    }

    var (first, firstAsked) = await _first(ask, have, pace, onProgress);
    if (kept != null &&
        (first.size != kept.size || first.sha256 != kept.sha256)) {
      // Another archive than the one kept (the release was packed again):
      // from the start.
      _forget(staging);
      kept = null;
      have = 0;
      received = 0;
      (first, firstAsked) = await _first(ask, 0, pace, onProgress);
    }
    size = first.size;
    final sha = first.sha256;
    final appName = first.appName;
    if (!appName.endsWith('.app') ||
        appName.startsWith('.') ||
        appName.contains('/')) {
      throw UpdateFailure('the download holds "$appName", not an app');
    }
    if (first.data.isEmpty && have < size) {
      throw const UpdateFailure('the download came back empty');
    }
    var through = have;
    void keep() => _Kept(
      version: version,
      size: size,
      sha256: sha,
      appName: appName,
      through: through,
    ).write(note);
    keep();

    final file = await part.open(mode: FileMode.append);
    // What came after [through] from a download that stopped isn't known
    // to be whole.
    await file.truncate(have);
    // One write at a time: a file takes one operation at once.
    var writing = Future<void>.value();
    Future<void> write(int offset, List<int> data) =>
        writing = writing.then((_) async {
          await file.setPosition(offset);
          await file.writeFrom(data);
        });
    // Pieces that came after one still on its way: where each starts, to
    // where it ends.
    final ahead = <int, int>{};
    Future<void> landed(int offset, int end) async {
      ahead[offset] = end;
      received += end - offset;
      final from = through;
      while (ahead.containsKey(through)) {
        through = ahead.remove(through)!;
      }
      if (through > from) {
        // On disk before the note says so; in turn with the writes.
        final upTo = through;
        await (writing = writing.then((_) => file.flush()));
        if (upTo == through) keep();
      }
      onProgress?.call(
        UpdateProgress(UpdateStage.downloading, received: received, size: size),
      );
    }

    // A backend from before FetchApp.max_bytes sends its own size, 1 MiB,
    // whatever is asked: then each piece is asked at that size.
    var fixed = false;
    // Where the next piece starts, and parts of pieces that came short.
    var next = have;
    final gaps = <(int, int)>[];
    Future<void> take(int offset, int asked, AppPiece piece) async {
      if (piece.preparing || piece.size != size || piece.sha256 != sha) {
        throw const _StartOver(
          'the update changed on the other machine while it downloaded',
        );
      }
      var data = piece.data;
      if (data.isEmpty) throw const UpdateFailure('the download was cut short');
      if (data.length > asked) {
        fixed = true;
        data = data.sublist(0, asked);
      }
      if (data.length < asked) {
        gaps.add((offset + data.length, asked - data.length));
      }
      await write(offset, data);
      await landed(offset, offset + data.length);
    }

    Object? failure;
    StackTrace? failureTrace;
    var busy = 0;
    Future<void> worker() async {
      while (failure == null && (gaps.isNotEmpty || next < size)) {
        // Whole MiBs go one at a time, each a long wait for the pings.
        if (fixed && busy > 0) return;
        final int offset;
        final int asked;
        if (gaps.isNotEmpty) {
          (offset, asked) = gaps.removeLast();
        } else {
          offset = next;
          asked = math.min(fixed ? _mostPerPiece : pace.next, size - offset);
          next += asked;
        }
        busy++;
        try {
          await take(offset, asked, await ask(offset, asked));
        } catch (error, trace) {
          failure ??= error;
          failureTrace ??= trace;
        } finally {
          busy--;
        }
      }
    }

    try {
      if (first.data.isNotEmpty) {
        fixed = first.data.length > firstAsked;
        final asked = math.min(first.data.length, size - have);
        next = have + asked;
        await take(have, asked, first);
      }
      await Future.wait([for (var i = 0; i < inFlight; i++) worker()]);
      await writing;
    } finally {
      await file.close();
    }
    if (failure case final failure?) {
      Error.throwWithStackTrace(failure, failureTrace!);
    }
    onProgress?.call(UpdateProgress(UpdateStage.checking, size: size));
    final length = part.lengthSync();
    if (length != size) {
      throw _StartOver('the download is incomplete ($length of $size bytes)');
    }
    final digest = await sha256.bind(part.openRead()).first;
    if (digest.toString() != sha.toLowerCase()) {
      throw const _StartOver('the download is damaged');
    }
    return appName;
  }

  /// The archive's piece from [offset], waiting while the other machine
  /// packs it, and how much was asked for.
  Future<(AppPiece, int)> _first(
    Future<AppPiece> Function(int offset, int maxBytes) ask,
    int offset,
    Pace pace,
    void Function(UpdateProgress)? onProgress,
  ) async {
    final started = DateTime.now();
    while (true) {
      final asked = pace.next;
      final piece = await ask(offset, asked);
      if (!piece.preparing) return (piece, asked);
      onProgress?.call(const UpdateProgress(UpdateStage.preparing));
      if (DateTime.now().difference(started) > patience) {
        throw const UpdateFailure(
          'the other machine took too long to prepare the update',
        );
      }
      await Future<void>.delayed(pollEvery);
    }
  }
}

/// Whether [connected] is true, or becomes so within [within].
Future<bool> _connects(ValueListenable<bool> connected, Duration within) {
  if (connected.value) return Future.value(true);
  final done = Completer<bool>();
  late final Timer timer;
  void changed() {
    if (!connected.value || done.isCompleted) return;
    timer.cancel();
    connected.removeListener(changed);
    done.complete(true);
  }

  timer = Timer(within, () {
    connected.removeListener(changed);
    if (!done.isCompleted) done.complete(false);
  });
  connected.addListener(changed);
  return done.future;
}
