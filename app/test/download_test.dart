// Download on the Mac (Matt, Bruno 41308): a viewer pane had Share… but no
// way to just download its file. Download, beside Share…, fetches it as
// Share… does (in pieces, with how much has come and Cancel) into the
// Mac's Downloads folder, beside what's there under a name of its own, and
// says where it went, with Show in Finder. Every viewer has it; a phone
// and an iPad save with Share… → Save to Files instead.
import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/share_files.dart';
import 'package:factory_app/viewers/video.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart' show OffsetLayer;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';
import 'panes_test.dart' show beside, snapshot;
import 'phone_sizes_test.dart' show loadGeist;
import 'video_test.dart' show FakePlayback;

const _plan = '/w/plan.md';
const _clip = '/w/clip.mp4';
const _archive = '/w/build.zip';
const _mb = 1024 * 1024;

FileContent _file(String path, List<int> data, {String mime = 'text/plain'}) =>
    FileContent(
      path: path,
      mime: mime,
      size: Int64(data.length),
      data: data,
      sha256: 'v1-${path.split('/').last}',
    );

/// Lets the disk and the reads do their work, until [done].
Future<void> _work(WidgetTester tester, bool Function() done) async {
  for (var i = 0; i < 1000; i++) {
    if (done()) return;
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 30)),
    );
    await tester.pump(const Duration(milliseconds: 30));
  }
  fail('It never came to that.');
}

/// With SHOTS set, the screen drawn to SHOTS/[name].png:
///   SHOTS=/tmp/download flutter test test/download_test.dart
Future<void> _shot(WidgetTester tester, String name) async {
  final folder = Platform.environment['SHOTS'];
  if (folder == null) return;
  final boundary = tester.binding.renderViews.first;
  final layer = boundary.debugLayer! as OffsetLayer;
  final image = await tester.runAsync(
    () => layer.toImage(Offset.zero & boundary.size, pixelRatio: 2),
  );
  final bytes = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory(folder).createSync(recursive: true);
  File('$folder/$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
}

bool _shows(String text) => find.text(text).evaluate().isNotEmpty;

Future<void> _tapLabel(WidgetTester tester, String label) async {
  final semantics = tester.ensureSemantics();
  await tester.tap(find.bySemanticsLabel(label));
  semantics.dispose();
  await tester.pump();
}

Future<Harness> _mac(WidgetTester tester, String kind, String path) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final harness = Harness(snapshot(layout: beside(kind, path)));
  harness.backend.files
    ..[_plan] = _file(_plan, 'Ship it.\n'.codeUnits)
    ..[_clip] = _file(_clip, List.filled(3 * _mb, 7), mime: 'video/mp4')
    ..[_archive] = _file(
      _archive,
      List.filled(1000, 3),
      mime: 'application/zip',
    );
  await harness.show(tester, size: const Size(1200, 900));
  await _work(tester, () => true);
  for (var i = 0; i < 10; i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 50)),
    );
    await tester.pump();
  }
  await settle(tester);
  return harness;
}

void main() {
  late Directory downloads;
  final shown = <List<String>>[];
  setUpAll(() async {
    await loadGeist();
    WidgetController.hitTestWarningShouldBeFatal = true;
  });
  tearDownAll(() => WidgetController.hitTestWarningShouldBeFatal = false);
  setUp(() {
    FakePlayback.made.clear();
    newVideoPlayback = FakePlayback.new;
    debugSharesFolder = Directory.systemTemp.createTempSync('shares-test-');
    downloads = Directory.systemTemp.createTempSync('downloads-test-');
    debugDownloadsFolder = downloads;
    shown.clear();
    showInFinder = (files) async =>
        shown.add([for (final file in files) file.path]);
  });
  tearDown(() {
    for (final folder in [debugSharesFolder, downloads]) {
      if (folder != null && folder.existsSync()) {
        folder.deleteSync(recursive: true);
      }
    }
    debugSharesFolder = null;
    debugDownloadsFolder = null;
  });

  for (final (kind, path, name) in [
    ('text', _plan, 'plan.md'),
    ('video', _clip, 'clip.mp4'),
    // One this app can't show: its pane says so, with Download all the same.
    ('archive', _archive, 'build.zip'),
  ]) {
    testWidgets('on a Mac, $kind: the viewer downloads its file to Downloads, '
        'saying where it went', (tester) async {
      final harness = await _mac(tester, kind, path);
      expect(find.bySemanticsLabel('Share…'), findsOneWidget);
      await _shot(tester, '$kind-header');
      await _tapLabel(tester, 'Download');
      await _work(tester, () => _shows('Downloaded'));
      await settle(tester);
      await _shot(tester, '$kind-downloaded');
      expect(find.text('$name is in your Downloads folder.'), findsOneWidget);
      final saved = File('${downloads.path}/$name');
      expect(saved.readAsBytesSync(), harness.backend.files[path]!.data);

      // Show in Finder shows it there, selected, and closes.
      await tester.tap(find.text('Show in Finder'));
      await tester.pump();
      expect(shown, [
        [saved.path],
      ]);
      expect(find.text('Downloaded'), findsNothing);
      // Nothing of it stays where it was fetched to.
      await _work(tester, () => debugSharesFolder!.listSync().isEmpty);
    });
  }

  testWidgets('downloaded again, it goes beside the first as "name 2"', (
    tester,
  ) async {
    await _mac(tester, 'text', _plan);
    for (final name in ['plan.md', 'plan 2.md', 'plan 3.md']) {
      await _tapLabel(tester, 'Download');
      await _work(tester, () => _shows('$name is in your Downloads folder.'));
      await tester.tap(find.text('Done'));
      await tester.pump();
    }
    expect(downloads.listSync().map((e) => e.uri.pathSegments.last).toSet(), {
      'plan.md',
      'plan 2.md',
      'plan 3.md',
    });
    expect(shown, isEmpty);
  });

  testWidgets('it shows how much has come, and Cancel leaves nothing in '
      'Downloads', (tester) async {
    final harness = await _mac(tester, 'video', _clip);
    final second = Completer<void>();
    harness.backend.holdRead = (request) =>
        request.path == _clip && request.offset > 0
        ? second.future
        : Future.value();
    await _tapLabel(tester, 'Download');
    await _work(tester, () => _shows('Loading… 1.0 MB of 3.1 MB'));
    expect(find.text('Download clip.mp4'), findsOneWidget);
    await tester.tap(find.text('Cancel').last);
    await tester.pump();
    second.complete();
    await _work(tester, () => debugSharesFolder!.listSync().isEmpty);
    expect(find.text('Download clip.mp4'), findsNothing);
    expect(downloads.listSync(), isEmpty);
  });

  testWidgets("Downloads it can't write to says so, and Try again saves it", (
    tester,
  ) async {
    await _mac(tester, 'text', _plan);
    // Where Downloads should be, a file.
    final blocked = File('${downloads.path}/Downloads')..writeAsStringSync('');
    debugDownloadsFolder = Directory(blocked.path);
    await _tapLabel(tester, 'Download');
    await _work(
      tester,
      () => find
          .textContaining("Couldn't save it to Downloads")
          .evaluate()
          .isNotEmpty,
    );
    expect(find.text('Download plan.md'), findsOneWidget);

    blocked.deleteSync();
    await tester.tap(find.text('Try again'));
    await _work(tester, () => _shows('Downloaded'));
    expect(File('${blocked.path}/plan.md').readAsStringSync(), 'Ship it.\n');
  });

  for (final pad in [false, true]) {
    testWidgets('on ${pad ? 'an iPad' : 'an iPhone'} there is no Download: '
        'Share… saves to Files', (tester) async {
      if (pad) {
        asIPad();
      } else {
        debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
      }
      final harness = Harness(snapshot(layout: beside('text', _plan)));
      harness.backend.files[_plan] = _file(_plan, 'Ship it.\n'.codeUnits);
      await harness.show(
        tester,
        size: pad ? const Size(1024, 1366) : const Size(393, 852),
      );
      await settle(tester);
      expect(canDownloadFiles, isFalse);
      expect(find.bySemanticsLabel('Download'), findsNothing);
    });
  }
}
