import 'dart:async';
import 'dart:io';
import 'dart:math' as math;

import 'package:crypto/crypto.dart';

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

/// Asks the backend for the archive of release [version] from [offset].
typedef FetchAppPiece = Future<AppPiece> Function(String version, int offset);

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
  ], "the new app's signature doesn't check out");

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

/// Updates this app from a Factory on another machine: it downloads the
/// release's app over the connection, checks it, and swaps it in once the
/// app has quit. (This Mac's Factory swaps its app itself.)
class AppUpdater {
  const AppUpdater({
    this.system = const AppSystem(),
    this.inFlight = 4,
    this.pollEvery = const Duration(seconds: 1),
    this.patience = const Duration(minutes: 10),
  });

  final AppSystem system;

  /// Pieces asked for at once.
  final int inFlight;

  /// How often to ask again while the other machine packs its app, and for
  /// how long at most.
  final Duration pollEvery;
  final Duration patience;

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
  /// why it can't, leaving nothing behind.
  Future<StagedApp> stage({
    required FetchAppPiece fetch,
    required String version,
    required String bundlePath,
    void Function(UpdateProgress progress)? onProgress,
  }) async {
    if (bundlePath.isEmpty) {
      throw const UpdateFailure("this app isn't in a bundle it can replace");
    }
    final parent = bundlePath.substring(0, bundlePath.lastIndexOf('/'));
    final staging = Directory('$parent/.${_name(bundlePath)}.update');
    try {
      if (staging.existsSync()) staging.deleteSync(recursive: true);
      staging.createSync();
    } on FileSystemException catch (error) {
      throw UpdateFailure(
        "can't write beside the app, in $parent: ${error.osError?.message ?? error.message}",
      );
    }
    try {
      final archive = '${staging.path}/app.tar.gz';
      final appName = await _download(fetch, version, archive, onProgress);
      onProgress?.call(const UpdateProgress(UpdateStage.checking));
      final unpacked = Directory('${staging.path}/unpacked')..createSync();
      await system.unpack(archive, appName, unpacked.path);
      File(archive).deleteSync();
      final app = '${unpacked.path}/$appName';
      if (!Directory(app).existsSync()) {
        throw UpdateFailure('the download has no $appName in it');
      }
      final ours = await system.bundleId(bundlePath);
      final theirs = await system.bundleId(app);
      if (theirs != ours) {
        throw UpdateFailure('the new app is $theirs, not this app ($ours)');
      }
      await system.verifySignature(app);
      return StagedApp(app: app, staging: staging.path);
    } catch (_) {
      try {
        staging.deleteSync(recursive: true);
      } on FileSystemException {
        // Nothing more to do; the next try starts by clearing it.
      }
      rethrow;
    }
  }

  /// Downloads the archive into [path], [inFlight] pieces at a time, and
  /// answers the bundle's name in it.
  Future<String> _download(
    FetchAppPiece fetch,
    String version,
    String path,
    void Function(UpdateProgress)? onProgress,
  ) async {
    final first = await _first(fetch, version, onProgress);
    final size = first.size;
    final sha = first.sha256;
    final appName = first.appName;
    if (!appName.endsWith('.app') ||
        appName.startsWith('.') ||
        appName.contains('/')) {
      throw UpdateFailure('the archive holds "$appName", not an app');
    }
    if (first.data.isEmpty && size > 0) {
      throw const UpdateFailure('the first piece of the app came back empty');
    }
    final step = first.data.length;
    var received = first.data.length;
    onProgress?.call(
      UpdateProgress(UpdateStage.downloading, received: received, size: size),
    );

    final file = await File(path).open(mode: FileMode.write);
    // One write at a time: a file takes one operation at once.
    var writing = Future<void>.value();
    Future<void> write(int offset, List<int> data) =>
        writing = writing.then((_) async {
          await file.setPosition(offset);
          await file.writeFrom(data);
        });
    var next = step;
    var failed = false;
    Future<void> worker() async {
      while (!failed && next < size) {
        final offset = next;
        next += step;
        final AppPiece piece;
        try {
          piece = await fetch(version, offset);
          if (piece.preparing || piece.size != size || piece.sha256 != sha) {
            throw const UpdateFailure(
              'the app changed on the other machine while it downloaded',
            );
          }
          if (piece.data.length != math.min(step, size - offset)) {
            throw const UpdateFailure('a piece of the app came back cut short');
          }
          await write(offset, piece.data);
        } catch (_) {
          failed = true;
          rethrow;
        }
        received += piece.data.length;
        onProgress?.call(
          UpdateProgress(
            UpdateStage.downloading,
            received: received,
            size: size,
          ),
        );
      }
    }

    try {
      await write(0, first.data);
      await Future.wait([for (var i = 0; i < inFlight; i++) worker()]);
      await writing;
    } finally {
      await file.close();
    }
    onProgress?.call(UpdateProgress(UpdateStage.checking, size: size));
    final length = File(path).lengthSync();
    if (length != size) {
      throw UpdateFailure('the download is $length bytes, not $size');
    }
    final digest = await sha256.bind(File(path).openRead()).first;
    if (digest.toString() != sha.toLowerCase()) {
      throw const UpdateFailure("the download's SHA-256 doesn't match");
    }
    return appName;
  }

  /// The archive's first piece, waiting while the other machine packs it.
  Future<AppPiece> _first(
    FetchAppPiece fetch,
    String version,
    void Function(UpdateProgress)? onProgress,
  ) async {
    final started = DateTime.now();
    while (true) {
      final piece = await fetch(version, 0);
      if (!piece.preparing) return piece;
      onProgress?.call(const UpdateProgress(UpdateStage.preparing));
      if (DateTime.now().difference(started) > patience) {
        throw const UpdateFailure(
          'the other machine took too long to pack the app',
        );
      }
      await Future<void>.delayed(pollEvery);
    }
  }
}
