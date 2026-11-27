// Sharing a file from the device: Share… fetches the whole
// original from the Claktory, a piece at a time with how much has come and
// Cancel, then opens the system's share sheet for it, and what was fetched
// is deleted once the sheet closes. From the viewer (More on a phone, the
// header elsewhere), a message's files (touch and hold; the Mac's right
// click) and Files' rows.
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/share_files.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart' show Text;
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, snapshot;
import 'ipad/as_ipad.dart';
import 'phone_sizes_test.dart' show loadGeist;

const _plan = '/w/plan.md';
const _clip = '/w/clip.mp4';

FileContent _file(String path, List<int> data, {String mime = 'text/plain'}) =>
    FileContent(
      path: path,
      mime: mime,
      size: Int64(data.length),
      data: data,
      sha256: 'v1-${path.split('/').last}',
    );

/// What the share sheet was asked to send: each file's path and what it
/// held then; [answer] is what it says back.
class _Sheet {
  _Sheet() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(const MethodChannel('factory/device'), (
          call,
        ) async {
          if (call.method != 'shareFiles') return null;
          final args = Map<Object?, Object?>.from(call.arguments as Map);
          final paths = [for (final p in args['paths']! as List) p as String];
          shared.add([
            for (final path in paths)
              (path: path, data: File(path).readAsBytesSync()),
          ]);
          mimes.add([for (final m in args['mimes']! as List) m as String]);
          rects.add(args['rect'] as List?);
          return answer;
        });
  }

  final shared = <List<({String path, List<int> data})>>[];
  final mimes = <List<String>>[];
  final rects = <List?>[];
  var answer = true;

  void close() => TestDefaultBinaryMessengerBinding
      .instance
      .defaultBinaryMessenger
      .setMockMethodCallHandler(const MethodChannel('factory/device'), null);
}

/// Lets the disk and the reads do their work: until [done] when given (so
/// a loaded machine only takes longer), else a while.
Future<void> _work(WidgetTester tester, [bool Function()? done]) async {
  for (var i = 0; i < (done == null ? 30 : 1000); i++) {
    if (done != null && done()) return;
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 30)),
    );
    // The clock too, so a sheet that opened meanwhile comes up.
    await tester.pump(const Duration(milliseconds: 30));
  }
  if (done != null && !done()) fail('It never came to that.');
}

bool _shows(String text, {bool contains = false}) =>
    (contains ? find.textContaining(text) : find.text(text))
        .evaluate()
        .isNotEmpty;

const _mb = 1024 * 1024;

Future<Harness> _phone(
  WidgetTester tester, {
  TargetPlatform platform = TargetPlatform.iOS,
  ViewNode? layout,
  List<Entry> entries = const [],
}) async {
  debugDefaultTargetPlatformOverride = platform;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final harness = Harness(snapshot(layout: layout));
  harness.backend
    ..appliesLayouts = true
    ..files[_plan] = _file(_plan, '# The plan\n\nShip it.\n'.codeUnits)
    ..files[_clip] = _file(
      _clip,
      List.filled(20 * 1024 * 1024, 7),
      mime: 'video/mp4',
    );
  harness.backend.transcript.load(Transcript(agentId: 'a1', entries: entries));
  tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
  await harness.show(tester, size: const Size(393, 852));
  await pumpFrames(tester);
  return harness;
}

/// The viewer's More, on a phone.
Future<void> _more(WidgetTester tester) async {
  final semantics = tester.ensureSemantics();
  await tester.tap(find.bySemanticsLabel('More').last);
  semantics.dispose();
}

Iterable<ReadFile> _reads(Harness harness, String path) => harness.backend.sent
    .where((m) => m.hasReadFile() && m.readFile.path == path)
    .map((m) => m.readFile);

Directory get _shares => sharesFolder;

void main() {
  late _Sheet sheet;
  // The real type, so a header's controls fit as they do in the app.
  setUpAll(() async {
    await loadGeist();
    // A tap that misses (a sheet still coming up) fails here, rather than
    // quietly leaving the share running.
    WidgetController.hitTestWarningShouldBeFatal = true;
  });
  tearDownAll(() => WidgetController.hitTestWarningShouldBeFatal = false);
  setUp(() {
    sheet = _Sheet();
    debugSharesFolder = Directory.systemTemp.createTempSync('shares-test-');
  });
  tearDown(() {
    sheet.close();
    final shares = debugSharesFolder;
    if (shares != null && shares.existsSync()) {
      shares.deleteSync(recursive: true);
    }
    debugSharesFolder = null;
  });

  for (final platform in [TargetPlatform.iOS, TargetPlatform.android]) {
    testWidgets("a viewer's More shares its file (${platform.name}): fetched "
        'a piece at a time with how much has come, sent, then kept a day', (
      tester,
    ) async {
      final harness = await _phone(tester, platform: platform);
      tester
          .widget<PhoneRoutesScope>(find.byType(PhoneRoutesScope))
          .routes
          .showFile('a1', _clip);
      await _work(tester);
      await pumpFrames(tester);
      // The second piece waits to be let through, as on a slow link.
      final second = Completer<void>();
      harness.backend.holdRead = (request) =>
          request.path == _clip && request.offset > 0
          ? second.future
          : Future.value();
      await _more(tester);
      await pumpFrames(tester);
      await tester.tap(find.text('Share…'));
      await pumpFrames(tester);
      await _work(tester, () => _shows('Loading… 1.0 MB of 21.0 MB'));
      await pumpFrames(tester);
      expect(find.text('Share clip.mp4'), findsOneWidget);
      expect(find.text('Loading… 1.0 MB of 21.0 MB'), findsOneWidget);
      expect(sheet.shared, isEmpty);
      second.complete();
      await _work(tester, () => sheet.shared.isNotEmpty);
      await pumpFrames(tester);

      // The whole original went to the sheet, from where each piece left
      // off, of the one version.
      final reads = _reads(harness, _clip).toList();
      expect(
        reads.map((r) => r.offset.toInt()).toList().sublist(reads.length - 20),
        [for (var mb = 0; mb < 20; mb++) mb * 1024 * 1024],
      );
      expect(reads.last.expectedSha256, 'v1-clip.mp4');
      final sent = sheet.shared.single.single;
      expect(sent.path, endsWith('/clip.mp4'));
      expect(sent.data.length, 20 * 1024 * 1024);
      expect(sheet.mimes.single, ['video/mp4']);
      expect(find.text('Share clip.mp4'), findsNothing);
      await _work(
        tester,
        () => File('${File(sent.path).parent.parent.path}/.sent').existsSync(),
      );
      // Kept a day for the app it went to, which may read it later (a
      // mail draft, Copy then paste), marked sent; swept then.
      final folder = File(sent.path).parent.parent;
      expect(File(sent.path).existsSync(), isTrue);
      expect(File('${folder.path}/.sent').existsSync(), isTrue);
      await tester.runAsync(() => folder.delete(recursive: true));
    });
  }

  testWidgets('Cancel stops it: nothing is sent, nothing stays', (
    tester,
  ) async {
    final harness = await _phone(tester);
    tester
        .widget<PhoneRoutesScope>(find.byType(PhoneRoutesScope))
        .routes
        .showFile('a1', _clip);
    await _work(tester);
    await pumpFrames(tester);
    final second = Completer<void>();
    harness.backend.holdRead = (request) =>
        request.path == _clip && request.offset > 0
        ? second.future
        : Future.value();
    await _more(tester);
    await pumpFrames(tester);
    await tester.tap(find.text('Share…'));
    await pumpFrames(tester);
    await _work(tester, () => _shows('Loading… 1.0 MB of 21.0 MB'));
    await pumpFrames(tester);
    expect(find.text('Loading… 1.0 MB of 21.0 MB'), findsOne);
    final folders = _shares.listSync().map((e) => e.path).toSet();
    await tester.tap(find.text('Cancel').last);
    await pumpFrames(tester);
    second.complete();
    await _work(
      tester,
      () => folders.every((folder) => !Directory(folder).existsSync()),
    );
    expect(sheet.shared, isEmpty);
    expect(find.text('Share clip.mp4'), findsNothing);
    // This share's folder is gone.
    for (final folder in folders) {
      expect(Directory(folder).existsSync(), isFalse, reason: folder);
    }
  });

  testWidgets('a read that fails says why, and Try again carries on from '
      'where it stopped', (tester) async {
    final harness = await _phone(
      tester,
      entries: [
        Entry(
          seq: Int64(1),
          user: UserText(
            text: 'The take',
            attachments: [
              Attachment(path: _clip, name: 'clip.mp4', size: Int64(9)),
            ],
          ),
        ),
      ],
    );
    var reads = 0;
    harness.backend.holdRead = (request) async {
      if (request.path == _clip && ++reads == 2) {
        throw const RequestFailure('The connection was lost.');
      }
    };
    await tester.longPress(find.text('clip.mp4'));
    await pumpFrames(tester);
    await tester.tap(find.text('Share…'));
    await pumpFrames(tester);
    await _work(tester, () => _shows('The connection was lost.'));
    await pumpFrames(tester);
    expect(find.text('The connection was lost.'), findsOneWidget);
    expect(sheet.shared, isEmpty);
    await tester.tap(find.text('Try again'));
    await _work(tester, () => sheet.shared.isNotEmpty);
    await pumpFrames(tester);
    expect(sheet.shared.single.single.data.length, 20 * 1024 * 1024);
    // Not from the start again: the first piece once, the second asked
    // for twice (failed, then again), then the rest.
    expect(
      [for (final r in _reads(harness, _clip)) r.offset.toInt()],
      [0, for (var mb = 1; mb < 20; mb++) mb * 1024 * 1024]
        ..insert(1, 1024 * 1024),
    );
  });

  testWidgets('given up after a long drop, it says only that, and carries '
      "on by itself once it's back (Sale's row 91)", (tester) async {
    final harness = await _phone(
      tester,
      entries: [
        Entry(
          seq: Int64(1),
          user: UserText(
            text: 'The take',
            attachments: [
              Attachment(path: _clip, name: 'clip.mp4', size: Int64(9)),
            ],
          ),
        ),
      ],
    );
    var reads = 0;
    harness.backend.holdRead = (request) async {
      if (request.path == _clip && ++reads == 3) {
        harness.backend.connected.value = false;
        throw const RequestFailure(
          "The connection dropped while this was loading and hasn't come "
          'back. Try again.',
        );
      }
    };
    await tester.longPress(find.text('clip.mp4'));
    await pumpFrames(tester);
    await tester.tap(find.text('Share…'));
    await pumpFrames(tester);
    await _work(tester, () => _shows('Try again.', contains: true));
    await pumpFrames(tester);
    expect(find.textContaining('Waiting for the connection'), findsNothing);
    expect(sheet.shared, isEmpty);

    // Back: on from where it got, no Try again.
    harness.backend.connected.value = true;
    await _work(tester, () => sheet.shared.isNotEmpty);
    expect(sheet.shared.single.single.data.length, 20 * 1024 * 1024);
    final offsets = [for (final r in _reads(harness, _clip)) r.offset.toInt()];
    expect(offsets.where((offset) => offset == 0), hasLength(1));
    expect(offsets.sublist(0, 4), [0, _mb, 2 * _mb, 2 * _mb]);
  });

  testWidgets('started offline, it names the Claktory, and carries on by '
      "itself once it's back (Abbacchio's row 1065)", (tester) async {
    final harness = await _phone(
      tester,
      entries: [
        Entry(
          seq: Int64(1),
          user: UserText(
            text: 'The take',
            attachments: [
              Attachment(path: _clip, name: 'clip.mp4', size: Int64(9)),
            ],
          ),
        ),
      ],
    );
    harness.backend.holdRead = (request) async {
      if (!harness.backend.connected.value) {
        throw const RequestFailure("Can't reach your Claktory right now.");
      }
    };
    await tester.longPress(find.text('clip.mp4'));
    await pumpFrames(tester);
    harness.backend.connected.value = false;
    await tester.tap(find.text('Share…'));
    await pumpFrames(tester);
    await _work(tester, () => _shows('Not connected to ', contains: true));
    await pumpFrames(tester);
    expect(find.textContaining('the backend'), findsNothing);
    final said = tester
        .widget<Text>(find.textContaining('Not connected to '))
        .data!;
    expect(said, isNot(contains('backend')));
    expect(said, endsWith('.'));
    // Nothing came yet: no empty bar under it.
    expect(find.text('Loading…'), findsNothing);
    expect(sheet.shared, isEmpty);

    harness.backend.connected.value = true;
    await _work(tester, () => sheet.shared.isNotEmpty);
    expect(sheet.shared.single.single.data.length, 20 * 1024 * 1024);
  });

  testWidgets("a message's files: touch and hold one for Share… or all of "
      'them, sent together', (tester) async {
    final harness = await _phone(
      tester,
      entries: [
        Entry(
          seq: Int64(1),
          user: UserText(
            text: 'Both takes',
            attachments: [
              Attachment(path: _plan, name: 'plan.md', size: Int64(20)),
              Attachment(path: _clip, name: 'clip.mp4', size: Int64(9)),
            ],
          ),
        ),
      ],
    );
    await tester.longPress(find.text('clip.mp4'));
    await pumpFrames(tester);
    expect(find.text('Share…'), findsOneWidget);
    // The second file waits, as on a slow link.
    final second = Completer<void>();
    harness.backend.holdRead = (request) =>
        request.path == _clip ? second.future : Future.value();
    await tester.tap(find.text('Share 2 files…'));
    await pumpFrames(tester);
    await _work(tester, () => _shows('File 2 of 2'));
    await pumpFrames(tester);
    expect(find.text('Share 2 files'), findsOneWidget);
    expect(find.text('File 2 of 2'), findsOneWidget);
    expect(find.text('clip.mp4'), findsWidgets);
    second.complete();
    await _work(tester, () => sheet.shared.isNotEmpty);
    await pumpFrames(tester);
    final sent = sheet.shared.single;
    expect(
      [for (final f in sent) f.path.split('/').last],
      ['plan.md', 'clip.mp4'],
    );
    expect(String.fromCharCodes(sent.first.data), '# The plan\n\nShip it.\n');
    // From the file touched (its rect, for an iPad's popover).
    expect(sheet.rects.single, hasLength(4));
  });

  for (final pad in [false, true]) {
    testWidgets('on ${pad ? 'an iPad' : 'a Mac'} the viewer has Share… in its '
        'header, the sheet pointing at it', (tester) async {
      if (pad) {
        asIPad();
      } else {
        debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
      }
      final harness = Harness(snapshot(layout: beside('text', _plan)));
      harness.backend.files[_plan] = _file(_plan, 'Ship it.\n'.codeUnits);
      await harness.show(
        tester,
        size: pad ? const Size(1024, 1366) : const Size(1200, 900),
      );
      await settle(tester);
      final semantics = tester.ensureSemantics();
      await tester.tap(find.bySemanticsLabel('Share…'));
      semantics.dispose();
      await _work(tester, () => sheet.shared.isNotEmpty);
      await pumpFrames(tester);
      expect(
        String.fromCharCodes(sheet.shared.single.single.data),
        'Ship it.\n',
      );
      final rect = sheet.rects.single!;
      expect(rect[2], greaterThan(0));
    });
  }

  testWidgets("on a phone a file's More starts with Share…", (tester) async {
    await _phone(tester);
    tester
        .widget<PhoneRoutesScope>(find.byType(PhoneRoutesScope))
        .routes
        .showFile('a1', _plan);
    await _work(tester);
    await pumpFrames(tester);
    final semantics = tester.ensureSemantics();
    await tester.tap(find.bySemanticsLabel('More').last);
    await pumpFrames(tester);
    semantics.dispose();
    // The sheet's first action.
    final actions = tester
        .widgetList<Text>(find.byType(Text))
        .map((text) => text.data)
        .where((label) => label == 'Share…' || label == 'Close everywhere')
        .toList();
    expect(actions.first, 'Share…');
    await tester.tap(find.text('Share…').last);
    await pumpFrames(tester);
    await _work(tester, () => sheet.shared.isNotEmpty);
    await pumpFrames(tester);
    expect(
      String.fromCharCodes(sheet.shared.single.single.data),
      '# The plan\n\nShip it.\n',
    );
  });

  testWidgets("on Android a PDF it can't show is still in a pane: Back, and "
      'Share… in More (Sale, Bruno 30855)', (tester) async {
    const report = '/w/report.pdf';
    final harness = await _phone(tester, platform: TargetPlatform.android);
    harness.backend.files[report] = _file(
      report,
      '%PDF-1.7 report'.codeUnits,
      mime: 'application/pdf',
    );
    tester
        .widget<PhoneRoutesScope>(find.byType(PhoneRoutesScope))
        .routes
        .showFile('a1', report);
    await _work(tester);
    await pumpFrames(tester);
    expect(
      find.text('To see this PDF, open it on a Mac, iPhone or iPad.'),
      findsOneWidget,
    );
    final semantics = tester.ensureSemantics();
    expect(find.bySemanticsLabel('Back'), findsWidgets);
    await tester.tap(find.bySemanticsLabel('More').last);
    await pumpFrames(tester);
    semantics.dispose();
    await tester.tap(find.text('Share…').last);
    await pumpFrames(tester);
    await _work(tester, () => sheet.shared.isNotEmpty);
    await pumpFrames(tester);
    expect(
      String.fromCharCodes(sheet.shared.single.single.data),
      '%PDF-1.7 report',
    );
    expect(sheet.mimes.single.single, 'application/pdf');
  });

  testWidgets('what a share cut short left goes at launch', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    await tester.runAsync(() async {
      final left = File('${_shares.path}/old/0/clip.mp4');
      await left.create(recursive: true);
      await sweepSharedFiles();
      expect(await left.exists(), isFalse);
    });
  });

  for (final platform in [TargetPlatform.iOS, TargetPlatform.android]) {
    testWidgets('a share sent somewhere stays a day, for the app it went to '
        '(${platform.name})', (tester) async {
      debugDefaultTargetPlatformOverride = platform;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      await tester.runAsync(() async {
        final shares = _shares;
        Future<File> share(String name, {bool sent = true}) async {
          final file = File('${shares.path}/$name/0/clip.mp4');
          await file.create(recursive: true);
          if (sent) await File('${shares.path}/$name/.sent').create();
          return file;
        }

        final old = await share('old');
        final fresh = await share('fresh');
        final cut = await share('cut', sent: false);
        // Sent two days ago.
        final made = DateTime.now().subtract(const Duration(days: 2));
        final stamp = [
          made.year,
          for (final part in [made.month, made.day, made.hour, made.minute])
            '$part'.padLeft(2, '0'),
        ].join();
        await Process.run('touch', ['-t', stamp, '${shares.path}/old/.sent']);
        try {
          await sweepSharedFiles();
          expect(await old.exists(), isFalse);
          expect(await fresh.exists(), isTrue);
          // Cut short: gone at launch, however new.
          expect(await cut.exists(), isFalse);
        } finally {
          await shares.delete(recursive: true);
        }
      });
    });
  }

  test("on Android shares go in the cache folder its host's file provider "
      'serves; elsewhere in the temporary folder', () {
    debugSharesFolder = null;
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    expect(sharesFolder.path, endsWith('/cache/claktory-share'));
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    expect(sharesFolder.path, '${Directory.systemTemp.path}/claktory-share');
    debugDefaultTargetPlatformOverride = null;
  });

  test("a file's name on the device can't pass for another (Fugo)", () {
    expect(shareName('/w/clip.mp4'), 'clip.mp4');
    expect(shareName('/w/invoice\u202Efdp.exe'), 'invoicefdp.exe');
    expect(shareName('/w/a\u200Bb\u2066c\uFEFF\n.txt'), 'abc.txt');
    for (final path in ['/w/', '/w/.', '/w/..', '/w/\u200B']) {
      expect(shareName(path), 'file', reason: path);
    }
    final long = shareName('/w/${'é' * 200}.mp4');
    expect(utf8.encode(long).length, lessThanOrEqualTo(255));
    expect(long, endsWith('é.mp4'));
  });
}
