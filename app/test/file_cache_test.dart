// Files kept on a device connected to a Claktory on another machine:
// opened again, a file shows from the copy kept here, asked for unless
// it's still that version, so nothing is brought over the link again; a
// changed one comes with the answer; offline, the copy shows,
// saying it may be out of date; past the size set for this device, the
// copies used longest ago go; Settings sets the size and clears them.
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:factory_app/connections.dart' show parsePairingCode;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/viewers/file_cache.dart';
import 'package:factory_app/viewers/file_cache_setting.dart';
import 'package:factory_app/viewers/video.dart';
import 'package:factory_app/voice_keep.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'pairing_test.dart' show until;
import 'panes_test.dart' show beside, snapshot;
import 'tunnel_test.dart' show FakeTunnel, FakeTunnels, backendWith;
import 'video_test.dart' show FakePlayback;
import 'viewers_test.dart' show png;

const _shot = '/w/shot.png';
const _clip = '/w/clip.mp4';

late Directory _folder;

/// Lets the disk and the decoder work, then the frames.
Future<void> _working(WidgetTester tester) async {
  for (var i = 0; i < 30; i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 20)),
    );
    await tester.pump();
  }
  await pumpFrames(tester);
}

Iterable<ReadFile> _reads(Harness harness, [int from = 0]) => harness
    .backend
    .sent
    .skip(from)
    .where((m) => m.hasReadFile())
    .map((m) => m.readFile);

FileContent _image(List<int> bytes, String sha) => FileContent(
  path: _shot,
  mime: 'image/png',
  size: Int64(bytes.length),
  data: bytes,
  sha256: sha,
);

/// Closes the pane, then shows it again: a new viewer.
Future<void> _reopen(
  WidgetTester tester,
  Harness harness, [
  String kind = 'image',
  String path = _shot,
]) async {
  harness.backend.workspace.apply(snapshot());
  await _working(tester);
  harness.backend.workspace.apply(snapshot(layout: beside(kind, path)));
  await _working(tester);
}

/// What's kept for the harness's Claktory, from the folder.
List<String> _keptFiles() => [
  for (final entry in _folder.listSync(recursive: true))
    if (entry is File &&
        !entry.path.endsWith('.json') &&
        !entry.path.endsWith('.part'))
      entry.path,
];

void main() {
  setUp(() {
    _folder = Directory.systemTemp.createTempSync('kept-files');
    keepsFilesOf = (_) => true;
    fileCacheFolder = () async => _folder.path;
  });

  tearDown(() {
    keepsFilesOf = (backend) => backend.target != null;
    FileCache.forgetAllInMemory();
    _folder.deleteSync(recursive: true);
  });

  testWidgets('opened again, a picture shows from the copy kept here, '
      'asked for unless it is still that version, with nothing read again', (
    tester,
  ) async {
    final bytes = await png(tester, 80, 40);
    final harness = Harness(snapshot(layout: beside('image', _shot)));
    harness.backend.files[_shot] = _image(bytes, 'v1');
    await harness.show(tester);
    await _working(tester);
    expect(find.byType(RawImage), findsOneWidget);
    expect(_reads(harness).where((r) => r.maxBytes == 0), hasLength(1));
    expect(_keptFiles(), hasLength(1));

    final before = harness.backend.sent.length;
    await _reopen(tester, harness);
    expect(find.byType(RawImage), findsOneWidget);
    final again = _reads(harness, before).toList();
    expect(again, hasLength(1));
    expect(again.single.unlessSha256, 'v1');
    expect(find.textContaining('may be out of date'), findsNothing);
  });

  testWidgets('a Claktory from before reads the picture as asked: still the '
      'kept version by its hash, it stays, with one read', (tester) async {
    final bytes = await png(tester, 80, 40);
    final harness = Harness(snapshot(layout: beside('image', _shot)));
    harness.backend
      ..files[_shot] = _image(bytes, 'v1')
      ..knowsUnless = false;
    await harness.show(tester);
    await _working(tester);

    final before = harness.backend.sent.length;
    await _reopen(tester, harness);
    expect(find.byType(RawImage), findsOneWidget);
    expect(_reads(harness, before), hasLength(1));
    expect(find.textContaining('may be out of date'), findsNothing);
    expect(_keptFiles(), hasLength(1));
  });

  testWidgets('a picture changed under the same name is read again and '
      'replaces its copy', (tester) async {
    final first = await png(tester, 80, 40);
    final second = await png(tester, 60, 60);
    final harness = Harness(snapshot(layout: beside('image', _shot)));
    harness.backend.files[_shot] = _image(first, 'v1');
    await harness.show(tester);
    await _working(tester);

    harness.backend.files[_shot] = _image(second, 'v2');
    final before = harness.backend.sent.length;
    await _reopen(tester, harness);
    final again = _reads(harness, before).toList();
    expect(again.map((r) => (r.maxBytes.toInt(), r.unlessSha256)), [
      (0, 'v1'),
    ], reason: 'one read, which brings the new version');
    expect(find.text('60 × 60 · ${second.length} bytes'), findsOneWidget);
    // Only the new version is kept.
    final kept = _keptFiles();
    expect(kept, hasLength(1));
    expect(File(kept.single).readAsBytesSync(), second);
  });

  testWidgets('offline, the copy shows, saying it may be out of date, and '
      'is checked once connected', (tester) async {
    final bytes = await png(tester, 80, 40);
    final harness = Harness(snapshot(layout: beside('image', _shot)));
    harness.backend.files[_shot] = _image(bytes, 'v1');
    await harness.show(tester);
    await _working(tester);

    harness.backend.connected.value = false;
    final before = harness.backend.sent.length;
    await _reopen(tester, harness);
    expect(find.byType(RawImage), findsOneWidget);
    expect(find.textContaining('may be out of date'), findsOneWidget);
    expect(_reads(harness, before), isEmpty);

    harness.backend.connected.value = true;
    await _working(tester);
    expect(find.textContaining('may be out of date'), findsNothing);
    expect(_reads(harness, before).map((r) => r.unlessSha256).toList(), ['v1']);
  });

  testWidgets('a phone keeps the preview made for its screen, and the '
      'original once it is shown', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final original = await png(tester, 800, 400);
    final preview = await png(tester, 400, 200);
    final harness = Harness(snapshot(layout: beside('image', _shot)));
    harness.backend.files[_shot] = _image(original, 'v1');
    harness.backend.previews[_shot] = FileContent(
      path: _shot,
      mime: 'image/webp',
      size: Int64(preview.length),
      data: preview,
      sha256: 'v1',
      preview: FilePreview.FILE_PREVIEW_MADE,
      pixelWidth: 400,
      pixelHeight: 200,
      originalSize: Int64(original.length),
      originalPixelWidth: 800,
      originalPixelHeight: 400,
    );
    await harness.show(tester, size: const Size(1200, 900));
    await _working(tester);
    expect(_keptFiles(), hasLength(1));
    final cache = FileCache.of(harness.backend)!;
    final kept = await tester.runAsync(
      () => cache.find(_shot, previewVariant((width: 900, height: 1200))),
    );
    expect(kept?.content.preview, FilePreview.FILE_PREVIEW_MADE);
  });

  testWidgets('past the size set for this device, the copies used longest '
      'ago go first', (tester) async {
    final harness = Harness(
      snapshot()..settings = (Settings()..fileCache['local'] = Int64(250)),
    );
    final cache = FileCache.of(harness.backend)!;
    expect(cache.limit, 250);
    FileContent file(String path, int size) => FileContent(
      path: path,
      size: Int64(size),
      data: List<int>.filled(size, 7),
      sha256: 'v-$path',
    );
    await tester.runAsync(() async {
      await cache.keepRead('/w/a', originalVariant, file('/w/a', 100));
      await Future<void>.delayed(const Duration(milliseconds: 5));
      await cache.keepRead('/w/b', originalVariant, file('/w/b', 100));
      await Future<void>.delayed(const Duration(milliseconds: 5));
      // a is used again: b is now the one used longest ago.
      await cache.touch((await cache.find('/w/a', originalVariant))!);
      await Future<void>.delayed(const Duration(milliseconds: 5));
      await cache.keepRead('/w/c', originalVariant, file('/w/c', 100));
    });
    final left = await tester.runAsync(
      () async => [
        for (final path in ['/w/a', '/w/b', '/w/c'])
          if (await cache.find(path, originalVariant) != null) path,
      ],
    );
    expect(left, ['/w/a', '/w/c']);
    expect(cache.used.value, 200);
    // More than it keeps at all: nothing.
    final big = await tester.runAsync(
      () => cache.keepRead('/w/d', originalVariant, file('/w/d', 300)),
    );
    expect(big, isNull);

    // Read again from the folder, as after a relaunch.
    FileCache.forgetAllInMemory();
    final again = FileCache.of(harness.backend)!;
    expect(await tester.runAsync(again.usage), 200);
  });

  testWidgets('a video opened again plays its kept copy, which stays when '
      'the pane closes', (tester) async {
    FakePlayback.made.clear();
    newVideoPlayback = FakePlayback.new;
    final harness = Harness(snapshot(layout: beside('video', _clip)));
    harness.backend
      ..rangedReads = true
      ..files[_clip] = FileContent(
        path: _clip,
        mime: 'video/mp4',
        size: Int64(3000),
        data: List<int>.filled(3000, 1),
        sha256: 'v1',
      );
    await harness.show(tester);
    await _working(tester);
    expect(FakePlayback.made.single.opened, startsWith(_folder.path));

    final before = harness.backend.sent.length;
    await _reopen(tester, harness, 'video', _clip);
    final again = _reads(harness, before).toList();
    expect(again.map((r) => (r.maxBytes.toInt(), r.expectedSha256)), [
      (1, 'v1'),
    ]);
    expect(FakePlayback.made, hasLength(2));
    expect(FakePlayback.made.last.opened!, FakePlayback.made.first.opened!);
    expect(File(FakePlayback.made.last.opened!).existsSync(), isTrue);
  });

  testWidgets('Settings sets the size for this device and clears what is '
      'kept', (tester) async {
    final harness = Harness(snapshot());
    final cache = FileCache.of(harness.backend)!;
    await tester.runAsync(
      () => cache.keepRead(
        '/w/a',
        originalVariant,
        FileContent(data: List<int>.filled(1500, 1), sha256: 'v1'),
      ),
    );
    await harness.show(tester, home: const FileCacheSetting());
    await _working(tester);
    expect(find.text('1 GB'), findsOneWidget);
    expect(find.textContaining('1.5 KB used.'), findsOneWidget);

    await tester.tap(find.text('1 GB'));
    await pumpFrames(tester);
    await tester.tap(find.text('100 MB').last);
    await pumpFrames(tester);
    expect(
      harness.backend.sent.where((m) => m.hasSetFileCache()).single,
      ClientMessage(setFileCache: SetFileCache(bytes: Int64(100000000))),
    );

    await tester.tap(find.text('Clear'));
    await _working(tester);
    expect(find.textContaining('0 bytes used.'), findsOneWidget);
    expect(_keptFiles(), isEmpty);
  });

  testWidgets('forgetting a Claktory deletes its copies', (tester) async {
    final harness = Harness(snapshot());
    final cache = FileCache.of(harness.backend)!;
    await tester.runAsync(() async {
      await cache.keepRead(
        '/w/a',
        originalVariant,
        FileContent(data: [1, 2, 3], sha256: 'v1'),
      );
      await FileCache.forget(cache.key);
    });
    expect(_keptFiles(), isEmpty);
  });

  test('turned away by its Claktory (removed from it, lost say), the '
      'device deletes what it kept of it; offline, it keeps it all', () async {
    final voices = Directory.systemTemp.createTempSync('kept-voices');
    final was = keptVoiceFolder;
    keptVoiceFolder = () async => voices.path;
    addTearDown(() {
      keptVoiceFolder = was;
      voices.deleteSync(recursive: true);
    });
    final target = parsePairingCode(
      'factory://?t=${'c' * 64}&name=mac-mini&e=${'a' * 64}',
    )!;
    final key = keptClaktoryKey(target);
    final files = Directory('${_folder.path}/$key')..createSync();
    File('${files.path}/kept.json').writeAsStringSync('{}');
    final other = Directory('${_folder.path}/other')..createSync();
    final recording = Directory('${voices.path}/1')..createSync();
    final voice = KeptVoice(
      path: '${recording.path}/Voice.m4a',
      duration: const Duration(seconds: 5),
      claktory: key,
      kind: KeptKind.agent,
      target: 'rivet',
      label: '',
      made: DateTime(2026, 11, 25),
      clientId: 'c1',
      uploadId: 'u1',
      requestId: 'r1',
    );
    File(voice.path).writeAsBytesSync([1, 2, 3]);
    File('${recording.path}/${KeptVoice.recordName}')
        .writeAsStringSync(jsonEncode(voice.toJson()));

    // Offline: nothing answers, and nothing goes.
    final gone = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final port = gone.port;
    await gone.close(force: true);
    final offline = backendWith(FakeTunnels(FakeTunnel(port)))
      ..deviceKey = (_) => 'k' * 64;
    offline.connectTo(target);
    unawaited(offline.run());
    await Future<void>.delayed(const Duration(milliseconds: 400));
    await offline.close();
    expect(offline.refused.value, isNull);
    expect(files.existsSync(), isTrue);
    expect(recording.existsSync(), isTrue);

    // Its Claktory says this device isn't one of its own any more.
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    server.listen((request) async {
      final ws = await WebSocketTransformer.upgrade(request);
      ws.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (!message.hasHello()) return;
        ws.add(
          ServerMessage(
            replyTo: message.id,
            error: Error(
              code: ErrorCode.ERROR_CODE_UNAUTHORIZED,
              message: 'This device was removed.',
            ),
          ).writeToBuffer(),
        );
      });
    });
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = (_) => 'k' * 64;
    backend.connectTo(target);
    unawaited(backend.run());
    await until(() => backend.refused.value != null);
    await until(() => !files.existsSync() && !recording.existsSync());
    expect(backend.refused.value, 'This device was removed.');
    expect(files.existsSync(), isFalse);
    expect(recording.existsSync(), isFalse);
    // Another Claktory's stay.
    expect(other.existsSync(), isTrue);
    await backend.close();
    await server.close(force: true);
  });

  test('a copy moved in is protected first, where it is; one that '
      "can't be is left where it is, kept nothing of", () async {
    final cache = FileCache.of(Harness(snapshot()).backend)!;
    final was = protectMovedIn;
    addTearDown(() => protectMovedIn = was);
    final asked = <String>[];
    var protects = true;
    protectMovedIn = (file) async {
      asked.add(file.path);
      return protects;
    };
    final scratch = Directory.systemTemp.createTempSync('download');
    addTearDown(() => scratch.deleteSync(recursive: true));
    final from = File('${scratch.path}/clip.mp4')..writeAsBytesSync([1, 2]);
    final content = FileContent(sha256: 'v1');

    protects = false;
    expect(await cache.adopt(_clip, originalVariant, content, from), isNull);
    expect(asked, [from.path]);
    expect(from.existsSync(), isTrue, reason: 'left for the viewer');
    expect(_keptFiles(), isEmpty);

    protects = true;
    final kept = await cache.adopt(_clip, originalVariant, content, from);
    expect(asked, [from.path, from.path]);
    expect(kept, isNotNull);
    expect(kept!.file.existsSync(), isTrue);
    expect(from.existsSync(), isFalse);
  });
}
