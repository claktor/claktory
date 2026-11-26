// The Mac's PDF viewer (viewers/pdf.dart): a "pdf" pane on a Mac reads the
// file in ranges from one version into the render cache, then shows it in
// the native viewer (app/macos/Runner/PaperViewer.swift); the header says
// which page, Find… finds, and a link asks before it opens.
import 'dart:convert';
import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:factory_app/find.dart';
import 'package:factory_app/ipad/ink_bridge.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/viewers/file_cache.dart';
import 'package:factory_app/viewers/pdf.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, showPanes, snapshot;

const _path = '/w/paper.pdf';

FileContent _pdf(String body) {
  final bytes = utf8.encode('%PDF-1.7\n$body');
  return FileContent(
    path: _path,
    mime: 'application/pdf',
    size: Int64(bytes.length),
    data: bytes,
    sha256: sha256.convert(bytes).toString(),
  );
}

/// The app's render cache channel: what it kept.
class _Cache {
  _Cache() {
    InkRenderCache.channel = const MethodChannel('test/pdf-cache');
    _messenger.setMockMethodCallHandler(InkRenderCache.channel, (call) async {
      final args = (call.arguments as Map?)?.cast<String, Object?>() ?? {};
      calls.add(call.method);
      switch (call.method) {
        case 'cacheLookup':
          return <String, Object?>{'file': null};
        case 'cacheBegin':
          _writing = [];
          _sha = args['sha256'] as String;
          return {'token': 't'};
        case 'cacheAppend':
          _writing.addAll(args['bytes'] as Uint8List);
          return null;
        case 'cacheFinish':
          if (sha256.convert(_writing).toString() != _sha) {
            throw PlatformException(code: 'hash_mismatch');
          }
          kept = utf8.decode(_writing);
          return {'file': '/cache/$_sha/paper.pdf'};
      }
      return null;
    });
    addTearDown(() {
      _messenger.setMockMethodCallHandler(InkRenderCache.channel, null);
      InkRenderCache.channel = const MethodChannel('claktory/ink');
    });
  }

  final calls = <String>[];
  var _writing = <int>[];
  var _sha = '';
  String? kept;
}

TestDefaultBinaryMessenger get _messenger =>
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;

/// The native viewer: the views made, and what each was asked.
class _Native {
  _Native() {
    _messenger.setMockMethodCallHandler(SystemChannels.platform_views, (
      call,
    ) async {
      if (call.method != 'create') return null;
      final args = call.arguments as Map<Object?, Object?>;
      made.add(args);
      final id = args['id'] as int;
      _messenger.setMockMethodCallHandler(MethodChannel('claktory/ink/$id'), (
        call,
      ) async {
        calls.add(call);
        final a = (call.arguments as Map?)?.cast<String, Object?>() ?? {};
        switch (call.method) {
          case 'search':
            final query = a['query'] as String;
            return {
              'query': query,
              'matches': query.isEmpty
                  ? []
                  : [
                      for (final page in [2, 5])
                        {
                          'page_1based': page,
                          'region': {'x': 0.1, 'y': 0.2, 'w': 0.3, 'h': 0.04},
                          'text': 'the $query here',
                        },
                    ],
              'truncated': false,
            };
          case 'goToPage':
            return {'page_1based': a['page_1based']};
        }
        return null;
      });
      addTearDown(
        () => _messenger.setMockMethodCallHandler(
          MethodChannel('claktory/ink/$id'),
          null,
        ),
      );
      return null;
    });
    addTearDown(
      () => _messenger.setMockMethodCallHandler(
        SystemChannels.platform_views,
        null,
      ),
    );
  }

  final made = <Map<Object?, Object?>>[];
  final calls = <MethodCall>[];

  /// The params the last view was made with.
  Map<Object?, Object?> get params =>
      const StandardMessageCodec().decodeMessage(
        ByteData.sublistView(made.last['params'] as Uint8List),
      ) as Map<Object?, Object?>;

  /// An event from the last view, as native sends it.
  Future<void> send(String method, Object? arguments) async {
    final id = made.last['id'] as int;
    await _messenger.handlePlatformMessage(
      'claktory/ink/$id',
      const StandardMethodCodec().encodeMethodCall(
        MethodCall(method, arguments),
      ),
      (_) {},
    );
  }
}

Future<Harness> _show(WidgetTester tester, FileContent file) async {
  final harness = await showPanes(
    tester,
    beside('pdf', _path),
    files: {_path: file},
  );
  tester.view.physicalSize = const Size(2000, 900);
  await settle(tester);
  return harness;
}

List<ReadFile> _reads(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasReadFile()) message.readFile,
];

void main() {
  final mac = TargetPlatformVariant.only(TargetPlatform.macOS);

  testWidgets(
    'on a Mac a PDF pane brings the file in ranges into the render cache '
    'and shows it in the native viewer, its page in the header',
    (tester) async {
      final cache = _Cache();
      final native = _Native();
      final chunk = pdfChunk;
      pdfChunk = 16;
      addTearDown(() => pdfChunk = chunk);
      final file = _pdf('a paper long enough to come in several reads');
      final harness = await _show(tester, file);

      final reads = _reads(harness);
      expect(reads.length, greaterThan(2));
      expect(reads.first.expectedSha256, isEmpty);
      for (final read in reads.skip(1)) {
        expect(read.expectedSha256, file.sha256);
      }
      expect(cache.kept, utf8.decode(file.data));
      expect(find.byType(PdfViewer), findsOneWidget);
      expect(find.byType(AppKitView), findsOneWidget);
      expect(
        find.text('To see this PDF, open it on a Mac, iPhone or iPad.'),
        findsNothing,
      );
      expect(native.made, hasLength(1));
      expect(native.made.single['viewType'], 'claktory/ink');
      final params = native.params;
      expect(params['surface'], 'pdf');
      expect(
        (params['document'] as Map)['file'],
        '/cache/${file.sha256}/paper.pdf',
      );
      expect((params['document'] as Map)['sha256'], file.sha256);
      // No drawing tools on a Mac.
      expect(find.text('Drawing tools'), findsNothing);

      await native.send('ready', {
        'surface': 'pdf',
        'source_sha256': file.sha256,
        'sha256_verified': true,
        'page_count': 12,
        'pages': [],
        'drawing_space': 'none',
      });
      await native.send('pageChanged', {'page_1based': 3, 'page_count': 12});
      await tester.pump();
      expect(find.text('Page 3 of 12'), findsOneWidget);
      expect(native.calls.first.method, 'attach');
    },
    variant: mac,
  );

  testWidgets(
    'from a Claktory on another machine, the PDF is kept with the files '
    'this device keeps (#446): opened again, only a one-byte read',
    (tester) async {
      final folder = Directory.systemTemp.createTempSync('kept-pdf');
      keepsFilesOf = (_) => true;
      fileCacheFolder = () async => folder.path;
      addTearDown(() {
        keepsFilesOf = (backend) => backend.target != null;
        FileCache.forgetAllInMemory();
        folder.deleteSync(recursive: true);
      });
      Future<void> working() async {
        for (var i = 0; i < 30; i++) {
          await tester.runAsync(
            () => Future<void>.delayed(const Duration(milliseconds: 20)),
          );
          await tester.pump();
        }
      }

      final cache = _Cache();
      final native = _Native();
      final file = _pdf('a paper');
      final harness = await _show(tester, file);
      await working();
      // Not the render cache: the device's own copies.
      expect(cache.calls, isEmpty);
      final shown = (native.params['document'] as Map)['file'] as String;
      expect(shown, startsWith(folder.path));
      expect(File(shown).readAsBytesSync(), file.data);

      final before = harness.backend.sent.length;
      harness.backend.workspace.apply(snapshot());
      await working();
      harness.backend.workspace.apply(snapshot(layout: beside('pdf', _path)));
      await working();
      final again = _reads(harness).skip(
        harness.backend.sent.take(before).where((m) => m.hasReadFile()).length,
      );
      expect(again.map((r) => r.maxBytes.toInt()), [1]);
      expect((native.params['document'] as Map)['file'], shown);
    },
    variant: mac,
  );

  testWidgets(
    "ink saved on an iPad shows over its pages, only read: each page's "
    'latest layer of the version shown, again as it is saved or deleted '
    'anywhere, and all of it when the view is made again (#368)',
    (tester) async {
      _Cache();
      final native = _Native();
      final file = _pdf('a paper');
      final harness = await _show(tester, file);
      List<Map<Object?, Object?>> loads() => [
        for (final call in native.calls)
          if (call.method == 'loadDrawing')
            for (final layer in (call.arguments as Map)['layers'] as List)
              layer as Map<Object?, Object?>,
      ];
      Future<void> ready() async {
        await native.send('ready', {
          'surface': 'pdf',
          'source_sha256': file.sha256,
          'sha256_verified': true,
          'page_count': 4,
          'pages': [],
          'drawing_space': 'none',
        });
        await pumpFrames(tester);
      }

      // No ink anywhere: the pages show as they always did.
      await ready();
      expect(loads(), isEmpty);
      expect(
        harness.backend.sent.where((m) => m.hasListInkLayers()).single,
        isNotNull,
      );

      InkLayer layer(
        String id,
        int page,
        int revision, {
        String? sha,
        String conflictOf = '',
        String kind = 'pdf_page',
      }) {
        final bytes = utf8.encode('ink $id $revision');
        final drawing = sha256.convert(bytes).toString();
        harness.backend.inkBlobs[drawing] = bytes;
        return InkLayer(
          id: id,
          path: _path,
          sourceSha256: sha ?? file.sha256,
          space: InkSpace(kind: kind, page1based: page),
          revision: revision,
          conflictOf: conflictOf,
          drawing: InkBlobRef(sha256: drawing, contentVersion: 2),
          preview: InkBlobRef(sha256: 'preview-$id-$revision'),
        );
      }

      void saved(List<InkLayer> layers) => harness.backend.inkLayerLists.value =
          InkLayerList(path: _path, layers: layers);

      final first = [
        layer('a', 1, 1),
        layer('a', 1, 2),
        layer('b', 3, 1),
        // A copy kept beside page 2's after a conflict, another version's
        // page 4 and an image's: none of them shows.
        layer('c', 2, 1, conflictOf: 'x'),
        layer('d', 4, 1, sha: 'another'),
        layer('e', 1, 9, kind: 'image'),
      ];
      saved(first);
      await pumpFrames(tester);
      var loaded = loads();
      expect([for (final l in loaded) l['page_1based']], [1, 3]);
      final one = loaded.first;
      expect(one['ink_layer_id'], 'a');
      expect(one['revision'], 2);
      expect(
        utf8.decode(
          base64Decode((one['drawing'] as Map)['bytes_base64'] as String),
        ),
        'ink a 2',
      );
      expect((one['drawing'] as Map)['content_version'], 2);

      // Saved again on the iPad: only that page loads again.
      native.calls.clear();
      saved([...first, layer('b', 3, 2)]);
      await pumpFrames(tester);
      loaded = loads();
      expect([for (final l in loaded) l['page_1based']], [3]);
      expect(loaded.single['revision'], 2);

      // Its layer deleted: the page shows none.
      native.calls.clear();
      saved([layer('b', 3, 2)]);
      await pumpFrames(tester);
      loaded = loads();
      expect(loaded.single['page_1based'], 1);
      expect(loaded.single['clear'], isTrue);

      // The view made again: what's saved, all of it.
      native.calls.clear();
      await ready();
      loaded = loads();
      expect([for (final l in loaded) l['page_1based']], [3]);
      expect(loaded.single['clear'], isNull);
    },
    variant: mac,
  );

  testWidgets("Search and ⌘F open the app's find bar over the page; Return and "
      '⇧Return go from match to match, Escape closes it', (tester) async {
    _Cache();
    final native = _Native();
    await _show(tester, _pdf('a paper'));
    await native.send('pageChanged', {'page_1based': 1, 'page_count': 9});
    await tester.pump();

    final bar = find.byType(FindBar);
    Finder field() =>
        find.descendant(of: bar, matching: find.byType(EditableText));
    expect(bar, findsNothing);
    await tester.tap(find.text('Search'));
    await tester.pump();
    await tester.pump();
    expect(bar, findsOneWidget);
    // Over the page, the PDF's own find.
    expect(
      find.ancestor(of: bar, matching: find.byType(PdfViewer)),
      findsOneWidget,
    );
    expect(find.text('Search this PDF'), findsOneWidget);
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'Find');

    await tester.enterText(field(), 'river');
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pump();
    final search = native.calls.lastWhere((c) => c.method == 'search');
    expect(search.arguments, {'query': 'river', 'max_results': 200});
    // The first match is shown.
    var go = native.calls.lastWhere((c) => c.method == 'goToPage');
    expect((go.arguments as Map)['page_1based'], 2);
    expect((go.arguments as Map)['region'], isNotNull);
    // The find's match: marked apart, and brought into sight.
    expect((go.arguments as Map)['match'], isTrue);
    expect(find.text('1 of 2'), findsOneWidget);
    // The count showing doesn't take the field away: typing goes on.
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'Find');
    final editable = tester.state<EditableTextState>(
      find.descendant(of: bar, matching: find.byType(EditableText)),
    );
    await tester.enterText(field(), 'rivers');
    await tester.enterText(field(), 'river');
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pump();
    expect(find.text('1 of 2'), findsOneWidget);
    expect(
      tester.state<EditableTextState>(
        find.descendant(of: bar, matching: find.byType(EditableText)),
      ),
      same(editable),
    );

    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    go = native.calls.lastWhere((c) => c.method == 'goToPage');
    expect((go.arguments as Map)['page_1based'], 5);
    expect(find.text('2 of 2'), findsOneWidget);

    await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
    await tester.pump();
    expect(find.text('1 of 2'), findsOneWidget);

    // The bar's arrows do the same, the field keeping the keys.
    await tester.tap(find.bySemanticsLabel('Next match'));
    await tester.pump();
    expect(find.text('2 of 2'), findsOneWidget);
    await tester.tap(find.bySemanticsLabel('Previous match'));
    await tester.pump();
    expect(find.text('1 of 2'), findsOneWidget);
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'Find');

    // ⌘G and ⇧⌘G pressed with the page focused: native sends them on.
    await native.send('findRequested', {'forward': true});
    await tester.pump();
    expect(find.text('2 of 2'), findsOneWidget);
    await native.send('findRequested', {'forward': false});
    await tester.pump();
    expect(find.text('1 of 2'), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pump();
    expect(bar, findsNothing);
    // The highlights go with it.
    expect(native.calls.last.method, 'search');
    expect((native.calls.last.arguments as Map)['query'], '');

    // ⌘F pressed with the page focused: native sends it on, and the
    // words come back, found again.
    await native.send('findRequested', {'forward': null});
    await tester.pump();
    await tester.pump();
    expect(bar, findsOneWidget);
    expect(native.calls.last.method, 'goToPage');
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pump();
    expect(bar, findsNothing);

    // ⌘F after a click on the page, in Flutter's keys.
    await tester.tapAt(tester.getCenter(find.byType(AppKitView)));
    await tester.pump();
    await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyF);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
    await tester.pump();
    await tester.pump();
    expect(bar, findsOneWidget);
    expect(
      find.ancestor(of: bar, matching: find.byType(PdfViewer)),
      findsOneWidget,
    );
  }, variant: mac);

  testWidgets(
    'a link in the PDF opens only when asked to; others are not offered',
    (tester) async {
      _Cache();
      final native = _Native();
      final opened = <Uri>[];
      final open = openPdfLink;
      openPdfLink = (url) async {
        opened.add(url);
        return true;
      };
      addTearDown(() => openPdfLink = open);
      await _show(tester, _pdf('a paper'));

      await native.send('linkTapped', {'url': 'file:///etc/passwd'});
      await tester.pump();
      expect(find.byKey(const ValueKey('pdf-link')), findsNothing);

      await native.send('linkTapped', {'url': 'https://example.com/a'});
      await tester.pump();
      expect(find.text('Open example.com?'), findsOneWidget);
      expect(opened, isEmpty);
      await tester.tap(find.text('Not now'));
      await tester.pump();
      expect(find.byKey(const ValueKey('pdf-link')), findsNothing);
      expect(opened, isEmpty);

      await native.send('linkTapped', {'url': 'mailto:ana@example.com'});
      await tester.pump();
      expect(find.text('Email ana@example.com?'), findsOneWidget);
      await tester.tap(find.text('Open'));
      await tester.pump();
      expect(opened, [Uri.parse('mailto:ana@example.com')]);
    },
    variant: mac,
  );

  testWidgets('a PDF too long to open says how long', (tester) async {
    _Cache();
    final native = _Native();
    await _show(tester, _pdf('a long paper'));
    await native.send('error', {
      'code': 'document_too_large',
      'recoverable': false,
      'page_count': 2400,
      'max_pages': 2000,
    });
    await tester.pump();
    expect(
      find.text(
        'This PDF has 2,400 pages, more than the 2,000 Claktory can open here.',
      ),
      findsOneWidget,
    );
    expect(find.byType(AppKitView), findsNothing);
  }, variant: mac);

  testWidgets('elsewhere than a Mac or an iPad the pane says where PDFs open', (
    tester,
  ) async {
    final harness = await _show(tester, _pdf('a paper'));
    expect(_reads(harness), isEmpty);
    expect(find.byType(PdfViewer), findsNothing);
    expect(
      find.text('To see this PDF, open it on a Mac, iPhone or iPad.'),
      findsOneWidget,
    );
  });
}
