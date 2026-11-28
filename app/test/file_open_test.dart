// Opening a file over a far link: a viewer with nothing to show past
// [loadingShowsAfter] says it's loading, one whose file comes sooner never
// flashes that; a copy this device kept shows at once, before the check
// that it's still the file comes back; and on a phone a file's page comes
// at once, before the layout it's opened in comes back with it. Each open
// tells diagnostics how it went.
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/viewers/file_cache.dart';
import 'package:factory_app/viewers/registry.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, snapshot;

const _notes = '/w/notes.md';

FileContent _text(String body, {String sha = 'v1'}) => FileContent(
  path: _notes,
  mime: 'text/markdown',
  size: Int64(utf8.encode(body).length),
  data: utf8.encode(body),
  sha256: sha,
);

final _loading = find.byKey(const ValueKey('loading-going'));

/// Lets the disk work, then the frames.
Future<void> _working(WidgetTester tester) async {
  for (var i = 0; i < 20; i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 20)),
    );
    await tester.pump();
  }
}

Iterable<ReadFile> _reads(Harness harness, [int from = 0]) => harness
    .backend
    .sent
    .skip(from)
    .where((m) => m.hasReadFile())
    .map((m) => m.readFile);

void main() {
  testWidgets('a file that has not come within a moment says it is '
      'loading, and shows once it comes', (tester) async {
    final read = Completer<void>();
    final harness = Harness(snapshot(layout: beside('text', _notes)));
    harness.backend
      ..files[_notes] = _text('# Notes\n\nShip it.\n')
      ..holdRead = (_) => read.future;
    await harness.show(tester);
    await tester.pump(loadingShowsAfter ~/ 2);
    expect(_loading, findsNothing);
    await tester.pump(loadingShowsAfter);
    expect(_loading, findsOneWidget);
    expect(find.text('Loading…'), findsOneWidget);

    read.complete();
    await tester.pump();
    await tester.pump();
    expect(_loading, findsNothing);
    expect(find.textContaining('Ship it.'), findsOneWidget);
  });

  testWidgets('a file that comes at once never flashes the loading state', (
    tester,
  ) async {
    final harness = Harness(snapshot(layout: beside('text', _notes)));
    harness.backend.files[_notes] = _text('# Notes\n\nShip it.\n');
    await harness.show(tester);
    for (var i = 0; i < 5; i++) {
      await tester.pump(loadingShowsAfter);
      expect(_loading, findsNothing);
    }
    expect(find.textContaining('Ship it.'), findsOneWidget);
  });

  group('kept on this device', () {
    late Directory folder;
    setUp(() {
      folder = Directory.systemTemp.createTempSync('kept-files');
      keepsFilesOf = (_) => true;
      fileCacheFolder = () async => folder.path;
    });
    tearDown(() {
      keepsFilesOf = (backend) => backend.target != null;
      FileCache.forgetAllInMemory();
      folder.deleteSync(recursive: true);
    });

    testWidgets('a copy kept here shows at once, before the check that '
        "it's still the file comes back", (tester) async {
      final harness = Harness(snapshot(layout: beside('text', _notes)));
      harness.backend.files[_notes] = _text('# Notes\n\nShip it.\n');
      await harness.show(tester);
      await _working(tester);
      expect(find.textContaining('Ship it.'), findsOneWidget);
      expect(harness.backend.viewerNotes, [
        'shown file=markdown from=read preview=0 bytes=18 size=18 bulk=0',
      ]);

      // Closed and opened again, over a link where the check takes long.
      final check = Completer<void>();
      harness.backend.holdRead = (request) =>
          request.unlessSha256.isNotEmpty ? check.future : Future.value();
      harness.backend.workspace.apply(snapshot());
      await _working(tester);
      final before = harness.backend.sent.length;
      harness.backend.workspace.apply(snapshot(layout: beside('text', _notes)));
      await _working(tester);
      expect(find.textContaining('Ship it.'), findsOneWidget);
      expect(_loading, findsNothing);
      expect(find.textContaining('may be out of date'), findsNothing);
      expect(
        harness.backend.viewerNotes.last,
        'shown file=markdown from=kept preview=0 bytes=0 size=18 bulk=0',
      );

      check.complete();
      await _working(tester);
      expect(
        harness.backend.viewerNotes.last,
        'checked file=markdown kept=current bulk=0',
      );
      expect(_reads(harness, before).map((r) => r.unlessSha256), [
        'v1',
      ], reason: 'still the file: nothing more is read');
    });

    testWidgets('a copy of a file changed since shows until the file comes '
        'in its place', (tester) async {
      final harness = Harness(snapshot(layout: beside('text', _notes)));
      harness.backend.files[_notes] = _text('# Notes\n\nShip it.\n');
      await harness.show(tester);
      await _working(tester);

      harness.backend.workspace.apply(snapshot());
      await _working(tester);
      harness.backend.files[_notes] = _text(
        '# Notes\n\nShip it on Friday.\n',
        sha: 'v2',
      );
      final full = Completer<void>();
      harness.backend.holdRead = (request) =>
          request.maxBytes == 0 ? full.future : Future.value();
      harness.backend.workspace.apply(snapshot(layout: beside('text', _notes)));
      await _working(tester);
      expect(find.textContaining('Ship it.'), findsOneWidget);

      final before = harness.backend.sent.length;
      full.complete();
      await _working(tester);
      expect(find.textContaining('Ship it on Friday.'), findsOneWidget);
      expect(
        harness.backend.viewerNotes,
        contains('checked file=markdown kept=changed bulk=0'),
      );
      expect(
        _reads(harness, before),
        isEmpty,
        reason: 'the new version came with the answer',
      );
    });

    testWidgets('a file closed before it came is kept all the same when it '
        'comes', (tester) async {
      final read = Completer<void>();
      final harness = Harness(snapshot(layout: beside('text', _notes)));
      harness.backend
        ..files[_notes] = _text('# Notes\n\nShip it.\n')
        ..holdRead = (_) => read.future;
      await harness.show(tester);
      await _working(tester);
      harness.backend.workspace.apply(snapshot());
      await _working(tester);
      read.complete();
      await _working(tester);

      harness.backend.holdRead = (_) => Completer<void>().future;
      harness.backend.workspace.apply(snapshot(layout: beside('text', _notes)));
      await _working(tester);
      expect(find.textContaining('Ship it.'), findsOneWidget);
    });

    testWidgets('closed before a changed file comes, the copy is still '
        'there to show next time; the read that came after it closed takes '
        'its place', (tester) async {
      final harness = Harness(snapshot(layout: beside('text', _notes)));
      harness.backend.files[_notes] = _text('# Notes\n\nShip it.\n');
      await harness.show(tester);
      await _working(tester);
      harness.backend.workspace.apply(snapshot());
      await _working(tester);

      harness.backend.files[_notes] = _text(
        '# Notes\n\nShip it on Friday.\n',
        sha: 'v2',
      );
      final full = Completer<void>();
      harness.backend.holdRead = (request) =>
          request.maxBytes == 0 ? full.future : Future.value();
      // Opened, found changed, and closed while the new version comes.
      harness.backend.workspace.apply(snapshot(layout: beside('text', _notes)));
      await _working(tester);
      harness.backend.workspace.apply(snapshot());
      await _working(tester);
      // Again: the copy at once, not a loading page.
      harness.backend.workspace.apply(snapshot(layout: beside('text', _notes)));
      await _working(tester);
      expect(find.textContaining('Ship it.'), findsOneWidget);
      expect(_loading, findsNothing);

      full.complete();
      await _working(tester);
      expect(find.textContaining('Ship it on Friday.'), findsOneWidget);
      harness.backend.workspace.apply(snapshot());
      await _working(tester);
      harness.backend.holdRead = (_) => Completer<void>().future;
      harness.backend.workspace.apply(snapshot(layout: beside('text', _notes)));
      await _working(tester);
      expect(find.textContaining('Ship it on Friday.'), findsOneWidget);
    });
  });

  testWidgets('on a phone a file opens as a page at once, its file read '
      'before the layout comes back with it, and is that pane after', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(snapshot()..selectedAgentId = 'a1');
    harness.backend.files[_notes] = _text('# Notes\n\nShip it.\n');
    tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
    addTearDown(tester.view.resetPadding);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);

    final routes = tester
        .widget<PhoneRoutesScope>(find.byType(PhoneRoutesScope))
        .routes;
    expect(routes.showFile('a1', _notes), isTrue);
    await pumpFrames(tester);
    final layout = harness.backend.sent.lastWhere((m) => m.hasSetLayout());
    expect(find.textContaining('Ship it.'), findsOneWidget);
    expect(_reads(harness), hasLength(1));

    // The layout comes back with the pane: the same page, not read again.
    harness.backend.workspace.apply(
      snapshot(layout: layout.setLayout.root)..selectedAgentId = 'a1',
    );
    await pumpFrames(tester);
    expect(find.textContaining('Ship it.'), findsOneWidget);
    expect(_reads(harness), hasLength(1));

    // Closed elsewhere afterwards, it goes.
    harness.backend.workspace.apply(snapshot()..selectedAgentId = 'a1');
    await pumpFrames(tester);
    expect(find.textContaining('Ship it.'), findsNothing);
  });
}
