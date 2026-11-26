import 'dart:async';
import 'dart:io';
import 'dart:math' as math;

import 'package:crypto/crypto.dart';
import 'package:factory_app/app_update.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';

/// A Factory's release archive served in pieces as FetchApp would: as many
/// bytes as asked, at most [step] (a backend's 1 MiB), or always [step]
/// from a backend from before FetchApp.max_bytes ([legacy]); counting how
/// many are asked for at once.
class Pieces {
  Pieces(
    this.archive, {
    this.step = 1 << 20,
    this.preparingFor = 0,
    this.legacy = false,
  });

  final List<int> archive;
  final int step;
  final bool legacy;

  /// How many times to answer `preparing` first.
  int preparingFor;
  String? sha256Override;

  /// Called before each piece is answered; what it throws, the piece does.
  void Function(int offset)? before;
  var asked = <int>[];
  var sizes = <int>[];
  var inFlight = 0;
  var mostInFlight = 0;

  Future<AppPiece> call(String version, int offset, {int maxBytes = 0}) async {
    expect(version, '2026.12.01-1200');
    if (preparingFor > 0) {
      preparingFor--;
      return const AppPiece(preparing: true);
    }
    asked.add(offset);
    sizes.add(maxBytes);
    inFlight++;
    mostInFlight = math.max(mostInFlight, inFlight);
    try {
      await Future<void>.delayed(const Duration(milliseconds: 2));
      before?.call(offset);
    } finally {
      inFlight--;
    }
    final most = legacy || maxBytes == 0 ? step : math.min(maxBytes, step);
    return AppPiece(
      size: archive.length,
      sha256: sha256Override ?? sha256.convert(archive).toString(),
      appName: 'Factory.app',
      data: archive.sublist(offset, math.min(offset + most, archive.length)),
    );
  }
}

/// The connection dropping under a piece, as the backend throws it.
class Dropped implements Exception {
  const Dropped();
}

/// Makes an ad-hoc signed bundle at [path] with the bundle identifier [id].
void makeApp(String path, String id) {
  Directory('$path/Contents/MacOS').createSync(recursive: true);
  File('/usr/bin/true').copySync('$path/Contents/MacOS/Factory');
  File('$path/Contents/Info.plist').writeAsStringSync('''
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>$id</string>
<key>CFBundleExecutable</key><string>Factory</string>
</dict></plist>
''');
  // Enough bytes, that don't compress, that the archive takes many pieces.
  final random = math.Random(7);
  File('$path/Contents/Resources/data.bin')
    ..createSync(recursive: true)
    ..writeAsBytesSync(List.generate(60000, (_) => random.nextInt(256)));
  final signed = Process.runSync('/usr/bin/codesign', [
    '--force',
    '--sign',
    '-',
    path,
  ]);
  expect(signed.exitCode, 0, reason: '${signed.stderr}');
}

/// A release archive of an app with [id] as publish.sh packs it; [tamper]
/// changes the bundle after it's signed.
List<int> archiveOf(
  Directory temp,
  String id, {
  bool tamper = false,
  bool linux = false,
}) {
  final release = Directory('${temp.path}/release-$id-$tamper-$linux')
    ..createSync();
  makeApp('${release.path}/Factory.app', id);
  // A Linux Factory serves the whole published Mac release.
  if (linux) File('${release.path}/factoryd').writeAsStringSync('backend');
  if (tamper) {
    File('${release.path}/Factory.app/Contents/Resources/data.bin')
        .writeAsStringSync('changed', mode: FileMode.append);
  }
  final archive = '${temp.path}/app-$id-$tamper-$linux.tar.gz';
  final packed = Process.runSync(
    '/usr/bin/tar',
    [
      '--no-xattrs',
      '-czf',
      archive,
      '-C',
      release.path,
      'Factory.app',
      if (linux) 'factoryd',
    ],
    environment: {'COPYFILE_DISABLE': '1'},
  );
  expect(packed.exitCode, 0, reason: '${packed.stderr}');
  return File(archive).readAsBytesSync();
}

void main() {
  final onMac = Platform.isMacOS;
  late Directory temp;
  late String bundle;
  // Pieces of 4 KiB, so an archive takes many.
  const updater = AppUpdater(
    pollEvery: Duration(milliseconds: 5),
    retryEvery: Duration(milliseconds: 5),
    firstPiece: 4096,
    leastPiece: 1024,
    mostPiece: 4096,
  );
  String stagingOf() => '${temp.path}/Applications/.Factory.app.update';

  setUp(() {
    temp = Directory.systemTemp.createTempSync('app-update');
    Directory('${temp.path}/Applications').createSync();
    bundle = '${temp.path}/Applications/Factory.app';
    if (onMac) makeApp(bundle, 'dev.factory.app');
  });
  tearDown(() => temp.deleteSync(recursive: true));

  Future<Object?> failure(Pieces pieces) async {
    try {
      await updater.stage(
        fetch: pieces.call,
        version: '2026.12.01-1200',
        bundlePath: bundle,
      );
    } on UpdateFailure catch (failure) {
      return failure.message;
    }
    return null;
  }

  test('downloads the app in pieces of the size it asks, two at a time, beside '
      'this one, and stages it once its size, hash, identity and signature '
      'check out', () async {
    final pieces = Pieces(archiveOf(temp, 'dev.factory.app'), preparingFor: 2);
    final progress = <UpdateProgress>[];
    final staged = await updater.stage(
      fetch: pieces.call,
      version: '2026.12.01-1200',
      bundlePath: bundle,
      onProgress: progress.add,
    );
    expect(staged.staging, stagingOf(), reason: "on the app's volume");
    expect(staged.app, '${staged.staging}/unpacked/Factory.app');
    expect(File('${staged.app}/Contents/Info.plist').existsSync(), isTrue);
    expect(Directory(staged.staging).listSync().map((e) => e.path), [
      '${staged.staging}/unpacked',
    ], reason: 'the archive and its note go once unpacked');
    final size = pieces.archive.length;
    expect(size, greaterThan(4096 * 6));
    expect(pieces.asked.toSet().length, pieces.asked.length);
    expect(pieces.asked.toSet(), {for (var at = 0; at < size; at += 4096) at});
    expect(
      pieces.sizes.take(pieces.sizes.length - 1),
      everyElement(4096),
      reason: 'each asks its size, the last what is left',
    );
    expect(pieces.mostInFlight, 2);
    expect(progress.first.stage, UpdateStage.preparing);
    final downloads = progress
        .where((p) => p.stage == UpdateStage.downloading)
        .toList();
    expect(downloads.last.received, size);
    expect(downloads.last.fraction, 1);
    expect(progress.last.stage, UpdateStage.checking);
  }, skip: !onMac);

  test('a download that stops keeps what it has, and the next carries on from '
      'there', () async {
    final pieces = Pieces(archiveOf(temp, 'dev.factory.app'));
    final size = pieces.archive.length;
    pieces.before = (offset) {
      if (offset >= 4096 * 4) throw const Dropped();
    };
    await expectLater(
      updater.stage(
        fetch: pieces.call,
        version: '2026.12.01-1200',
        bundlePath: bundle,
      ),
      throwsA(isA<Dropped>()),
    );
    final part = File('${stagingOf()}/app.tar.gz.part');
    expect(part.lengthSync(), greaterThanOrEqualTo(4096 * 4));
    expect(File('${stagingOf()}/download.json').existsSync(), isTrue);

    pieces
      ..before = null
      ..asked = [];
    final progress = <UpdateProgress>[];
    final staged = await updater.stage(
      fetch: pieces.call,
      version: '2026.12.01-1200',
      bundlePath: bundle,
      onProgress: progress.add,
    );
    expect(
      pieces.asked,
      everyElement(greaterThanOrEqualTo(4096 * 4)),
      reason: 'what came before is kept',
    );
    expect(pieces.asked.toSet(), {
      for (var at = pieces.asked.first; at < size; at += 4096) at,
    });
    expect(progress.first.received, greaterThanOrEqualTo(4096 * 5));
    expect(File('${staged.app}/Contents/Info.plist').existsSync(), isTrue);
  }, skip: !onMac);

  test('a dropped connection is waited for, and the download carries on by '
      'itself once it is back', () async {
    final pieces = Pieces(archiveOf(temp, 'dev.factory.app'));
    final connected = ValueNotifier(true);
    var dropped = false;
    pieces.before = (offset) {
      if (offset == 4096 * 3 && !dropped) {
        dropped = true;
        connected.value = false;
        Timer(const Duration(milliseconds: 50), () => connected.value = true);
        throw const Dropped();
      }
    };
    final progress = <UpdateProgress>[];
    final staged = await updater.stage(
      fetch: pieces.call,
      version: '2026.12.01-1200',
      bundlePath: bundle,
      connected: connected,
      onProgress: progress.add,
    );
    expect(dropped, isTrue);
    final waiting = progress.firstWhere((p) => p.stage == UpdateStage.waiting);
    expect(waiting.size, pieces.archive.length);
    expect(waiting.received, greaterThanOrEqualTo(4096 * 3));
    expect(pieces.asked.where((at) => at == 4096 * 3), hasLength(2));
    expect(File('${staged.app}/Contents/Info.plist').existsSync(), isTrue);
  }, skip: !onMac);

  test('relayed, it goes in small pieces and gently; direct again, at the '
      "link's pace (friend bug #51)", () async {
    final pieces = Pieces(archiveOf(temp, 'dev.factory.app'));
    final relayed = ValueNotifier(true);
    // 1 KiB at a time, 20 KiB a second, while relayed.
    const gentle = AppUpdater(
      pollEvery: Duration(milliseconds: 5),
      retryEvery: Duration(milliseconds: 5),
      firstPiece: 4096,
      leastPiece: 1024,
      mostPiece: 4096,
      relayedPiece: 1024,
      relayedRate: 20 * 1024,
    );
    final watch = Stopwatch()..start();
    Duration? turnedDirect;
    pieces.before = (offset) {
      // The path turns direct once 8 KiB have been asked for.
      if (offset >= 8 * 1024 && relayed.value) {
        relayed.value = false;
        turnedDirect = watch.elapsed;
      }
    };
    final staged = await gentle.stage(
      fetch: pieces.call,
      version: '2026.12.01-1200',
      bundlePath: bundle,
      relayed: relayed,
    );
    expect(File('${staged.app}/Contents/Info.plist').existsSync(), isTrue);
    final relayedSizes = pieces.sizes.take(8).toList();
    expect(relayedSizes, everyElement(lessThanOrEqualTo(1024)));
    // 8 KiB at 20 KiB a second: the first piece at once, the rest spaced.
    expect(
      turnedDirect,
      greaterThanOrEqualTo(const Duration(milliseconds: 300)),
    );
    expect(
      pieces.sizes.skip(8),
      contains(4096),
      reason: 'direct again, pieces grow back to the most',
    );
  }, skip: !onMac);

  test('from a backend that sends 1 MiB whatever is asked, pieces go one at a '
      'time at its size', () async {
    final pieces = Pieces(
      archiveOf(temp, 'dev.factory.app'),
      step: 8192,
      legacy: true,
    );
    final staged = await updater.stage(
      fetch: pieces.call,
      version: '2026.12.01-1200',
      bundlePath: bundle,
    );
    expect(File('${staged.app}/Contents/Info.plist').existsSync(), isTrue);
    expect(pieces.mostInFlight, 1);
    expect(pieces.asked.skip(1), everyElement((int at) => at % 8192 == 0));
  }, skip: !onMac);

  test('a kept download of another archive (the release packed again) starts '
      'over', () async {
    final pieces = Pieces(archiveOf(temp, 'dev.factory.app'));
    pieces.before = (offset) {
      if (offset >= 4096 * 4) throw const Dropped();
    };
    await expectLater(
      updater.stage(
        fetch: pieces.call,
        version: '2026.12.01-1200',
        bundlePath: bundle,
      ),
      throwsA(isA<Dropped>()),
    );
    final again = Pieces(archiveOf(temp, 'dev.factory.app', linux: true));
    final staged = await updater.stage(
      fetch: again.call,
      version: '2026.12.01-1200',
      bundlePath: bundle,
    );
    expect(again.asked, contains(0));
    expect(File('${staged.app}/Contents/Info.plist').existsSync(), isTrue);
  }, skip: !onMac);

  test('an archive that changes while it downloads (packed again) is '
      'downloaded again from the start', () async {
    final pieces = Pieces(archiveOf(temp, 'dev.factory.app'));
    var changed = false;
    pieces.before = (offset) {
      pieces.sha256Override = offset == 4096 * 2 && !changed ? 'f' * 64 : null;
      if (offset == 4096 * 2) changed = true;
    };
    final staged = await updater.stage(
      fetch: pieces.call,
      version: '2026.12.01-1200',
      bundlePath: bundle,
    );
    expect(pieces.asked.where((at) => at == 0), hasLength(2));
    expect(File('${staged.app}/Contents/Info.plist').existsSync(), isTrue);
  }, skip: !onMac);

  test("from a Linux Factory, whose archive is the whole Mac release, only the "
      'app is unpacked', () async {
    final pieces = Pieces(archiveOf(temp, 'dev.factory.app', linux: true));
    final staged = await updater.stage(
      fetch: pieces.call,
      version: '2026.12.01-1200',
      bundlePath: bundle,
    );
    expect(
      Directory('${staged.staging}/unpacked').listSync().map((e) => e.path),
      [staged.app],
    );
  }, skip: !onMac);

  test(
    'a download whose hash is off is downloaded again once, then refused and '
    'left nowhere',
    () async {
      final pieces = Pieces(archiveOf(temp, 'dev.factory.app'))
        ..sha256Override = '0' * 64;
      expect(await failure(pieces), 'the download is damaged');
      expect(pieces.asked.where((at) => at == 0), hasLength(2));
      expect(
        Directory('${temp.path}/Applications').listSync().map((e) => e.path),
        [bundle],
      );
    },
    skip: !onMac,
  );

  test('another app is refused', () async {
    final pieces = Pieces(archiveOf(temp, 'com.example.other'));
    expect(
      await failure(pieces),
      'the download is a different app (com.example.other, not dev.factory.app)',
    );
  }, skip: !onMac);

  test('an app changed after it was signed is refused', () async {
    final pieces = Pieces(archiveOf(temp, 'dev.factory.app', tamper: true));
    expect(
      await failure(pieces),
      startsWith("the new app's signature isn't valid"),
    );
    expect(
      Directory('${temp.path}/Applications/.Factory.app.update').existsSync(),
      isFalse,
    );
  }, skip: !onMac);

  test(
    'an app not in a bundle, or a folder it may not write, says so',
    () async {
      bundle = '';
      expect(
        await failure(Pieces(const [])),
        "this copy of the app can't replace itself",
      );
      bundle = '/System/Factory.app';
      expect(
        await failure(Pieces(const [])),
        startsWith("can't save the update next to the app, in /System"),
      );
    },
  );

  test(
    'the swap waits for the app to quit, puts the new bundle in place, '
    'clears the staging folder and opens it',
    () async {
      final pieces = Pieces(archiveOf(temp, 'dev.factory.app'));
      final staged = await updater.stage(
        fetch: pieces.call,
        version: '2026.12.01-1200',
        bundlePath: bundle,
      );
      File('$bundle/Contents/old').writeAsStringSync('the old app');
      // Notes what it was asked to open beside the bundle it opens (its last
      // argument). macOS checks a new executable the first time it runs, which
      // can take 20s, so the opener is one script that never changes, kept for
      // every run: only the first on a machine waits.
      final opened = '$bundle.opened';
      final opener = '${Directory.systemTemp.path}/factory-test-opener.sh';
      const script =
          '#!/bin/sh\nfor last; do :; done\necho "\$@" > "\$last.opened"\n';
      if (!File(opener).existsSync() ||
          File(opener).readAsStringSync() != script) {
        File(opener).writeAsStringSync(script);
        Process.runSync('/bin/chmod', ['+x', opener]);
      }
      // Stands in for this app, quitting a moment after the swap starts.
      final running = await Process.start('/bin/sleep', ['0.5']);
      await const AppSystem().startSwap(
        staged,
        app: bundle,
        pid: running.pid,
        opener: opener,
      );
      await Future<void>.delayed(const Duration(milliseconds: 200));
      expect(
        File('$bundle/Contents/old').existsSync(),
        isTrue,
        reason: 'it waits while the app runs',
      );
      await running.exitCode;
      // Generous: a machine under load, or macOS checking the opener the first
      // time it runs, can take seconds.
      for (var i = 0; i < 600 && !File(opened).existsSync(); i++) {
        await Future<void>.delayed(const Duration(milliseconds: 100));
      }
      expect(File(opened).readAsStringSync().trim(), bundle);
      expect(File('$bundle/Contents/old').existsSync(), isFalse);
      expect(File('$bundle/Contents/Resources/data.bin').existsSync(), isTrue);
      expect(Directory(staged.staging).existsSync(), isFalse);
      expect(Directory('$bundle.updating').existsSync(), isFalse);
      final verified = Process.runSync('/usr/bin/codesign', [
        '--verify',
        '--deep',
        '--strict',
        bundle,
      ]);
      expect(verified.exitCode, 0, reason: '${verified.stderr}');
    },
    skip: !onMac,
    timeout: const Timeout(Duration(minutes: 2)),
  );
}
